//! The content writer: every write to the content goes through
//! [`ContentWriter`].
//!
//! In: the builder's calls, one at a time, with their styles, text and
//! sizes. Out: a finished [`Content`] and a [`BuildReport`].
//! Start at: [`ContentWriter::new`], then the calls in `writer_calls`.
//!
//! - `writer_calls`: the builder's calls, from `text` and `open` to `finish`.
//! - `writer_first_letter`: the `::first-letter` box and where its letter ends.
//! - `writer_text`: text and white space, collapsed, transformed and written.
//! - `writer_nodes`: the container stack, nodes, items and the limit checks.
//! - `writer_lower`: lowers each node's styles into facts.
//! - `writer_check`: the debug build's checks of the content's invariants.
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

use super::{ContentWriter, FirstLetter, InitialLetterUse, Mirror, Open};
use alloc::boxed::Box;
use alloc::string::String;

use super::collapse::Collapser;
use super::memo::StyleMemo;
use super::transform::Transforms;
use super::{BlockFacts, Content, ItemId, NodeId, NodeKey, NodeKind};
use crate::build::{BuildOptions, BuildReport};
use crate::data::{HashIndex, Id, Table, TextOffset, define_id, heap_bytes};
use crate::style::ComputedBlockStyle;

/// The most the content may hold: the ids' own limits and the text's,
/// unless a test asks for less.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContentLimits {
    /// Bytes of text.
    pub(super) text: usize,
    /// Nodes, the block included.
    pub(super) nodes: usize,
    /// Items.
    pub(super) items: usize,
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
    pub(super) struct OpenId(u32);
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
        let BuildOptions { map_source } = options;
        let (root, first_line) = (block.style, block.first_line);
        let facts = BlockFacts::new(block, root);
        #[cfg(any(debug_assertions, test))]
        super::facts_check::block_is(&facts, root);
        content.clear(facts, map_source);
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
            last_char: ' ',
            text_node: None,
            math_text: None,
            math_key: None,
            open_item: None,
            source: 0,
            run_unit: None,
            continues: None,
            annotations: 0,
            ruby: None,
            flattened_rubies: 0,
            ruby_depth: 0,
            reserved: 0,
            first_line: first_line.is_some(),
            mirror: if first_line.is_some() {
                Mirror::Waiting
            } else {
                Mirror::Done
            },
            first_letter: FirstLetter::Unarmed,
            at_start: true,
            initial_letter: false,
            lead_trim: None,
            block_trims: false,
            full: false,
            limits: ContentLimits {
                nodes: limits.nodes.min(NodeId::MAX),
                items: limits.items.min(ItemId::MAX),
                text: limits.text.min(TextOffset::MAX),
            },
            report: BuildReport::default(),
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
            writer.lead_trim = Some(TextOffset::new(0));
            writer.block_trims = true;
        }
        writer
    }
}
