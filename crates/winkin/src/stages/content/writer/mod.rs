//! The content writer. Every write to the content goes through
//! [`ContentWriter`].
//!
//! In: the builder's calls with their styles, text and sizes. Out: a
//! finished [`Content`] and a [`BuildReport`].
//! Start at: [`ContentWriter::new`], then `calls` and `text`.
//!
//! - `calls`: every builder call except text, from `open` to `finish`.
//! - `first_letter`: arms `::first-letter` and writes its box.
//! - `text`: text and white space, collapsed, transformed and written.
//! - `nodes`: the container stack, nodes, items and limit checks.
//! - `lower`: lowers node styles into facts.
//! - `check`: debug checks of the finished content's invariants.
//!
//! The writer keeps the content's invariants: nodes in pre-order with their
//! item ranges, items tiling the text, the limits and the flags.
//!
//! **Limits.** The text holds at most [`TextOffset::MAX`] bytes, and nodes
//! and items at most what their ids can name. What doesn't fit is dropped,
//! whole characters and whole nodes at a time, and counted in the report;
//! the content stays valid. An open box reserves its close item, so items
//! always bracket.
//!
//! **A collapsed space is inserted where its run began**, before any opaque
//! items (box edges, floats, ruby marks, `<wbr>`) written while the run was
//! pending. Their offsets shift by one byte; their ids don't change. Only
//! U+200B can be written while a run is pending, so this moves few bytes.

mod calls;
#[cfg(debug_assertions)]
mod check;
mod first_letter;
mod lower;
mod nodes;
mod text;

use alloc::string::String;

use super::Lists;
use super::collapse::Collapser;
use super::map::MapUnitId;
use super::memo::{StyleKey, StyleMemo};
use super::transform::Transforms;
use super::{
    BlockFacts, BoxFactsId, Content, ContentStaging, ItemId, ItemKind, NodeId, NodeKey, NodeKind,
    TextFactsId, TextFitFacts,
};
use crate::build::{BuildOptions, BuildReport};
use crate::context::Context;
use crate::data::{HashIndex, Id, Table, TextOffset, define_id, heap_bytes};
use crate::style::{
    ComputedBlockStyle, ComputedStyle, InitialLetter, TextFitKind, VerticalAlign, WhiteSpaceTrim,
};

/// The most the content may hold. Tests may lower these.
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
    /// The limits of the ids and of [`TextOffset`].
    pub(crate) const MAX: Self = Self {
        text: TextOffset::MAX,
        nodes: NodeId::MAX,
        items: ItemId::MAX,
    };

    /// Returns these limits with at most `text` bytes of text.
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

/// The content writer's working memory, kept on the context so a warm
/// build allocates nothing.
///
/// The writer takes it from the context in [`ContentWriter::new`] and puts
/// it back in [`ContentWriter::finish`]. After the build, font selection and
/// measurement read its [`Lists`].
pub(crate) struct ContentScratch {
    /// The open containers, innermost last; the block is not on it.
    open: Table<OpenId, Open>,
    /// Segmenter scratch for capitalize (see `transform`).
    words: String,
    /// Lowered styles (see `memo`).
    memo: StyleMemo,
    /// Atomic inlines by the hash of their keys, to detect a shared key.
    atomic_keys: HashIndex,
    lists: Lists,
    /// Staging tables for a new layout's content.
    staging: ContentStaging,
}

impl ContentScratch {
    /// Creates empty scratch, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            open: Table::new(),
            words: String::new(),
            memo: StyleMemo::new(),
            atomic_keys: HashIndex::new(),
            lists: Lists::new(),
            staging: ContentStaging::new(),
        }
    }

    /// Returns the build's interned style lists.
    pub(crate) fn lists(&self) -> &Lists {
        &self.lists
    }

    /// Empties everything but the staging tables, keeping the capacity.
    fn clear(&mut self) {
        self.open.clear();
        self.words.clear();
        self.memo.clear();
        self.atomic_keys.clear();
        self.lists.clear();
    }
}

impl Default for ContentScratch {
    fn default() -> Self {
        Self::new()
    }
}

heap_bytes! {
    ContentScratch { open, words, memo, atomic_keys, lists, staging }
}

define_id! {
    /// Names an entry in the writer's stack of open containers, outermost
    /// first.
    struct OpenId(u32);
}

/// U+FFFC OBJECT REPLACEMENT CHARACTER, which stands for an atomic inline.
const OBJECT: &str = "\u{FFFC}";

/// The used value of `initial-letter` for a node being lowered.
///
/// A block has at most one initial letter, an inline box at its start (CSS
/// Inline 3, section 7.3.1); elsewhere the used value is `normal`. So any
/// style that keeps the property is the initial letter's, and later stages
/// size it from its facts alone.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum InitialLetterUse {
    /// The style doesn't set it.
    Unset,
    /// The box may be the initial letter. The property is kept and
    /// `vertical-align` becomes `baseline`, since it doesn't apply to an
    /// initial letter (CSS Inline 3, section 7.5.1).
    Kept,
    /// The used value is `normal`.
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

    /// Applies the used `initial-letter` to the keyed `style`.
    fn apply_key(self, style: &mut StyleKey) {
        match self {
            Self::Unset => {}
            Self::Kept => style.line.vertical_align = VerticalAlign::Baseline,
            Self::Normal => style.line.initial_letter = InitialLetter::NONE,
        }
    }
}

/// A node's fact ids in one variant (see `facts`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct NodeFacts {
    text: TextFactsId,
    box_: BoxFactsId,
}

/// A node's lowered styles: its facts in both variants and its text
/// transforms.
#[derive(Copy, Clone, Debug)]
struct LoweredNode {
    own: NodeFacts,
    /// The first-line facts, when the block has `::first-line`.
    first_line: Option<NodeFacts>,
    /// The first-line style differs from the node's own.
    restyles: bool,
    /// The transforms for text written in the node.
    transforms: Transforms,
}

/// The state of the first-line text mirror (see `first_line`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Mirror {
    /// The block has `::first-line`, but no first-paragraph text has had a
    /// differing first-line transform yet; nothing is mirrored.
    Waiting,
    /// Every write to the first paragraph is mirrored.
    Writing,
    /// The first paragraph has ended, or the block has no `::first-line`.
    Done,
}

/// The state of `::first-letter`.
///
/// The armed styles are held inline, a few hundred bytes. Boxing them would
/// allocate in every build with a first letter.
#[allow(clippy::large_enum_variant)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum FirstLetter {
    /// Not requested yet, and nothing written would precede it.
    Unarmed,
    /// Requested. The key and keyed styles wait for the first text holding
    /// a letter, which splits the box off. The styles are lowered then, when
    /// the parent and whether it is the initial letter are known.
    Armed {
        key: NodeKey,
        style: StyleKey,
        first_line: Option<StyleKey>,
    },
    /// The closed box holds the punctuation a text ended with. The letter is
    /// sought in what follows; if none comes, the box is made plain.
    Punctuation { node: NodeId },
    /// Found, ruled out, or no longer possible.
    Done,
}

impl FirstLetter {
    /// Whether the armed letter's own or first-line style is `math-auto`,
    /// so text nodes must be measured for it.
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

/// The source text node `math-auto` is measuring.
///
/// `math-auto` maps a text node only if it is one character, which is known
/// only when the node ends, so a lone character is held back.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum MathText {
    /// No text node is being measured.
    None,
    /// One character so far, held unwritten until the node continues or
    /// ends.
    Held(NodeKey, char),
    /// Longer than one character; its text is written as it comes.
    Long(NodeKey),
}

/// A container opened and not yet closed.
///
/// Each entry caches what calls inside it need, so no call searches the
/// stack: the node that owns text written inside, and for ruby, a link to
/// the next ruby entry out.
#[derive(Copy, Clone, Debug)]
struct Open {
    /// Its node, or `None` if it was dropped at a limit. A dropped entry
    /// stays on the stack so it, not the box outside, takes its close.
    node: Option<NodeId>,
    kind: ContainerKind,
    /// Its opening item.
    item: ItemId,
    trim: WhiteSpaceTrim,
    /// Its node, or for a dropped entry the enclosing container's node; the
    /// block outside every container.
    container: NodeId,
    /// The transforms for text written inside; a dropped entry has its
    /// container's.
    transforms: Transforms,
    /// For a ruby container, annotation or flattened ruby, the next such
    /// entry out, if any.
    outer: Option<OpenId>,
    /// An anonymous ruby container around an annotation outside any ruby;
    /// it closes with the annotation.
    anonymous: bool,
    /// For a ruby container, the closing item of its last annotation, which
    /// the next annotation flags.
    annotation_close: Option<ItemId>,
}

/// The kind of an open container.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ContainerKind {
    /// An inline box.
    Box,
    /// A ruby container.
    Ruby,
    /// A ruby annotation.
    Annotation,
    /// A ruby container written as a box, inside an annotation or past the
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

    /// Whether it is linked into the ruby chain the ruby calls follow.
    fn is_ruby(self) -> bool {
        matches!(self, Self::Ruby | Self::Annotation | Self::FlattenedRuby)
    }
}

/// The only writer of the content.
///
/// Its state groups mostly belong to one file each: `text_node`, `trim` and
/// `first_line` to `text`, `ruby` to `calls`, `letters` to `first_letter`,
/// and `room` to `nodes`.
pub(crate) struct ContentWriter<'a> {
    content: &'a mut Content,
    /// The context; its font context interns family lists.
    cx: &'a mut Context,
    /// The working memory, taken from the context while writing.
    scratch: ContentScratch,
    /// The transforms for text outside every container.
    block_transforms: Transforms,
    collapser: Collapser,
    text_node: TextNode,
    ruby: RubyNesting,
    letters: Letters,
    trim: Trim,
    first_line: FirstLine,
    room: Room,
}

/// The text node being written, and the text before it.
struct TextNode {
    /// The current text node; text with the same key continues it until
    /// anything else is written.
    node: Option<NodeId>,
    /// The node's text item, once it has one.
    item: Option<ItemId>,
    /// Source bytes of the text node consumed so far, kept or collapsed.
    /// The offset map counts in these bytes.
    source: u32,
    /// The offset map unit of the pending white space run, which takes the
    /// run's space if it is written.
    run_unit: Option<MapUnitId>,
    /// The key of the text node `::first-letter` split, and the source
    /// offset after the letter, while the next node may be its rest.
    continues: Option<(NodeKey, u32)>,
    /// The source text node `math-auto` is measuring.
    math: MathText,
    /// The last character written, or U+0020 if something else came after.
    ///
    /// Capitalize reads it, as Chrome reads `PreviousCharacter`. A `<br>`
    /// leaves `\n`, a `<wbr>` U+200B, and an atomic inline or float a
    /// space, as in Chrome.
    last_char: char,
}

/// The open ruby containers and annotations.
struct RubyNesting {
    /// The innermost ruby container, annotation or flattened ruby on the
    /// stack; each links to the next one out.
    innermost: Option<OpenId>,
    /// Open annotations, so their items are flagged.
    annotations: usize,
    /// Flattened ruby containers still owed an `end_ruby`. One already
    /// closed by an annotation of its column still consumes its
    /// `end_ruby`, which then closes nothing.
    flattened: usize,
    /// Real ruby containers open, bounded by `MAX_RUBY_DEPTH`.
    depth: usize,
}

/// The state of `::first-letter` and the initial letter.
struct Letters {
    first_letter: FirstLetter,
    /// Every item so far is an opening box edge, a float or absolute
    /// anchor, or a text item. While the text is also empty, a box opening
    /// now is a first child all the way up and may take `initial-letter`.
    at_start: bool,
    /// A box has taken `initial-letter`; no later or inner box can.
    initial_letter: bool,
}

/// The block's trims of its kept white space
/// (`white-space-trim: discard-inner`).
struct Trim {
    /// Where the leading trim would cut, just after the last segment break.
    /// Set only while the block trims and only white space has been
    /// written.
    lead: Option<TextOffset>,
    /// Whether the block trims at both ends. Trims cut at kept segment
    /// breaks, so the offset map starts a unit at each and no unit spans a
    /// cut.
    ends: bool,
}

/// The block's `::first-line` state.
struct FirstLine {
    /// Whether the block has `::first-line`, so every node has a first-line
    /// style.
    styled: bool,
    /// The state of the first-line text mirror.
    mirror: Mirror,
}

/// The room left in the content, and what was dropped.
struct Room {
    limits: ContentLimits,
    /// Close items reserved for the open containers.
    reserved: usize,
    /// The text reached its limit; nothing more is written.
    full: bool,
    report: BuildReport,
}

impl<'a> ContentWriter<'a> {
    /// Clears `content` and starts it with the block's node, keyed `key`
    /// and styled by `block` and its `::first-line` style.
    ///
    /// Takes the working memory from `cx`. Records the offset map if
    /// `options` asks.
    pub(crate) fn new(
        content: &'a mut Content,
        cx: &'a mut Context,
        key: NodeKey,
        block: &ComputedBlockStyle<'_>,
        options: BuildOptions,
        limits: ContentLimits,
    ) -> Self {
        let BuildOptions {
            map_source,
            percentage_basis,
        } = options;
        // A non-finite basis resolves percentages as zero.
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
        let mut scratch = cx.take_content_scratch();
        scratch.clear();
        scratch.staging.lend(content);
        let mut writer = Self {
            content,
            cx,
            scratch,
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
        // The fact tables are empty, so the block's facts come first. An
        // initial letter is an inline box, so the block's `initial-letter`
        // is `normal`.
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
