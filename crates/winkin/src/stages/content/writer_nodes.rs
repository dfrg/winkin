//! The writer's bookkeeping: the container stack, nodes, items and room.

use core::iter;

use super::collapse::Event;
use super::writer::OpenId;
use super::{
    BoxFlags, ContainerKind, Content, ContentFlags, ContentWriter, Item, ItemFlags, ItemId,
    ItemKind, LoweredNode, Node, NodeId, NodeKey, NodeKind, Open,
};
use crate::data::{TextOffset, make_text_room};
use crate::work;

impl ContentWriter<'_> {
    /// Pushes `open` on the stack, linking a ruby container or annotation to
    /// the next one out.
    ///
    /// The stack holds as many containers as an `OpenId` names. No caller can
    /// reach that in memory. One past it would be left off, and the container
    /// outside it would take its close.
    pub(super) fn push_open(&mut self, mut open: Open) {
        let links = open.kind.is_ruby();
        if links {
            open.outer = self.ruby;
        }
        match self.stack.push(open) {
            Some(id) if links => self.ruby = Some(id),
            Some(_) => {}
            None => debug_assert!(false, "no caller opens 2^32 containers at once"),
        }
    }

    /// Returns the open ruby containers and annotations, innermost first,
    /// with where each is on the stack.
    ///
    /// The ruby calls follow this chain of links and never look at the boxes
    /// between.
    pub(super) fn ruby_chain(&self) -> impl Iterator<Item = (OpenId, Open)> + '_ {
        let mut at = self.ruby;
        iter::from_fn(move || {
            work::step();
            let id = at?;
            let open = *self.stack.get(id)?;
            at = open.outer;
            Some((id, open))
        })
    }

    /// Closes the innermost container.
    pub(super) fn pop(&mut self) {
        work::step();
        if self.math_text.is_some() {
            self.end_text();
        }
        let Some(open) = self.stack.pop() else {
            return;
        };
        if open.kind.is_ruby() {
            self.ruby = open.outer;
        }
        let Some(node) = open.node else {
            return;
        };
        self.end_text();
        self.collapser.step(Event::Close {
            trim: open.trim,
            opened: open.item,
        });
        self.reserved = self.reserved.saturating_sub(1);
        if self
            .push_item(open.kind.close_item_kind(), node, ItemFlags::NONE)
            .is_none()
        {
            debug_assert!(false, "a close has its item reserved");
        }
        if open.kind == ContainerKind::Annotation {
            self.annotations = self.annotations.saturating_sub(1);
        }
        if open.kind == ContainerKind::Ruby {
            self.ruby_depth -= 1;
        }
        self.end_leaf(node);
    }

    /// Returns the current text node, if it is the caller's text node `key`.
    pub(super) fn text_node_keyed(&self, key: NodeKey) -> Option<NodeId> {
        self.text_node
            .filter(|&node| self.content.nodes.key(node) == key)
    }

    /// Ends the current text node, if any: it takes no more text.
    pub(super) fn end_text(&mut self) {
        if self.math_text.is_some() {
            self.end_math_text();
        }
        self.math_key = None;
        if let Some(node) = self.text_node.take() {
            self.end_leaf(node);
        }
        self.open_item = None;
    }

    /// Ends the source text node `math-auto` measures: a held character is
    /// written as the whole of it.
    pub(super) fn end_math_text(&mut self) {
        if let Some((key, ch)) = self.math_text.take() {
            self.write_text_call(key, ch.encode_utf8(&mut [0; 4]), true);
        }
        self.math_key = None;
    }

    /// Records that `node` has all its items.
    pub(super) fn end_leaf(&mut self, node: NodeId) {
        let end = self.content.items.next_id();
        if let Some(slot) = self.content.nodes.get_mut(node) {
            slot.end = end;
        }
    }

    /// Returns the innermost open container's node, or the block's.
    ///
    /// Each stack entry keeps it, so containers dropped at a limit cost
    /// nothing to look past.
    pub(super) fn container(&self) -> NodeId {
        self.stack
            .last()
            .map_or(NodeId::BLOCK, |open| open.container)
    }

    /// Appends a node of `kind` with the facts its styles were lowered into
    /// (`lowered`). Returns `None` if the node table is full.
    pub(super) fn push_node(
        &mut self,
        kind: NodeKind,
        lowered: LoweredNode,
        key: NodeKey,
    ) -> Option<NodeId> {
        if !self.has_node_room() {
            return None;
        }
        let parent = if kind == NodeKind::Block {
            NodeId::BLOCK
        } else {
            self.container()
        };
        let own = lowered.own;
        let first = lowered.first_line.unwrap_or(own);
        let first_item = self.content.items.next_id();
        let nodes = &mut self.content.nodes;
        let id = nodes.nodes.push(Node {
            key,
            text: own.text,
            first_line_text: first.text,
            box_: own.box_,
            first_line_box: first.box_,
            first_item,
            end: first_item,
            parent,
            kind,
        })?;
        self.note_first_line(own, first, lowered.restyles);
        if kind.is_inline_box() {
            self.content.flags.insert(ContentFlags::INLINE_BOXES);
            // The one box that keeps `initial-letter` is the block's
            // initial letter.
            let Content { block, facts, .. } = &mut *self.content;
            if block.initial_letter.is_none()
                && facts.box_facts(own.box_).has(BoxFlags::INITIAL_LETTER)
            {
                block.initial_letter = Some(id);
            }
        }
        self.content.record(|map| map.open_node());
        self.continues = None;
        Some(id)
    }

    /// Appends an empty item at the end of the text, leaving room for the
    /// closes owed. Returns `None` if no item fits.
    pub(super) fn push_item(
        &mut self,
        kind: ItemKind,
        node: NodeId,
        flags: ItemFlags,
    ) -> Option<ItemId> {
        if !kind.is_close() && !self.has_item_room(1) {
            return None;
        }
        if kind != ItemKind::Text || flags.contains(ItemFlags::GENERATED) {
            self.open_item = None;
        }
        // Text is looked for in the text itself: an item that holds none
        // yet may stay empty.
        if !matches!(
            kind,
            ItemKind::Open | ItemKind::Float | ItemKind::Absolute | ItemKind::Text
        ) {
            self.at_start = false;
        }
        let flags = if self.annotations > 0 {
            flags.union(ItemFlags::ANNOTATION)
        } else {
            flags
        };
        let at = TextOffset::new(self.content.text.len());
        self.content.items.push(Item {
            start: at,
            end: at,
            node,
            kind,
            flags,
        })
    }

    pub(super) fn has_node_room(&self) -> bool {
        self.content.nodes.len() < self.limits.nodes
    }

    pub(super) fn has_item_room(&self, count: usize) -> bool {
        self.content.items.len() + self.reserved + count <= self.limits.items
    }

    pub(super) fn has_text_room(&self, bytes: usize) -> bool {
        !self.full && self.content.text.len() + bytes <= self.limits.text
    }

    /// Reserves room in the text for `bytes` more, up to its limit.
    ///
    /// A text call hands in at most what it writes, since collapsing only
    /// shortens it. Reserving first makes a node's text one allocation, not
    /// one per doubling as its words arrive. It reserves exactly `bytes` in
    /// an empty text, doubles past the length where they do not fit, and does
    /// nothing where they do, as `Table::reserve` does. If the allocator
    /// refuses, it does nothing.
    pub(super) fn reserve_text(&mut self, bytes: usize) {
        let text = &mut self.content.text;
        let bytes = bytes.min(self.limits.text.saturating_sub(text.len()));
        make_text_room(text, bytes);
    }
}
