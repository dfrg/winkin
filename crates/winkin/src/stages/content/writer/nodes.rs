//! The container stack, nodes, items and room checks.

use core::{iter, mem};

use super::OpenId;
use super::{ContainerKind, ContentWriter, LoweredNode, MathText, Open};
use crate::data::TextOffset;
use crate::stages::content::collapse::Event;
use crate::stages::content::{
    BoxFlags, Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind, KeyedBox, Node, NodeId,
    NodeKey, NodeKind,
};
use crate::style::WhiteSpaceTrim;
use crate::work;

impl ContentWriter<'_> {
    /// Pushes `open` on the stack, linking a ruby entry to the next one out.
    ///
    /// The stack is bounded by `OpenId` (2^32 entries), which memory cannot
    /// reach. An entry past it would be left off, and the container outside
    /// would take its close.
    pub(super) fn push_open(&mut self, mut open: Open) {
        let links = open.kind.is_ruby();
        if links {
            open.outer = self.ruby.innermost;
        }
        match self.scratch.open.push(open) {
            Some(id) if links => self.ruby.innermost = Some(id),
            Some(_) => {}
            None => debug_assert!(false, "no caller opens 2^32 containers at once"),
        }
    }

    /// Returns the open ruby entries with their stack ids, innermost first.
    ///
    /// The chain skips the boxes between, so ruby calls cost the same at
    /// any box depth.
    pub(super) fn ruby_chain(&self) -> impl Iterator<Item = (OpenId, Open)> + '_ {
        let mut at = self.ruby.innermost;
        iter::from_fn(move || {
            work::step();
            let id = at?;
            let open = *self.scratch.open.get(id)?;
            at = open.outer;
            Some((id, open))
        })
    }

    /// Closes the innermost container.
    pub(super) fn pop(&mut self) {
        work::step();
        if matches!(self.text_node.math, MathText::Held(..)) {
            self.end_text();
        }
        let Some(open) = self.scratch.open.pop() else {
            return;
        };
        if open.kind.is_ruby() {
            self.ruby.innermost = open.outer;
        }
        let Some(node) = open.node else {
            return;
        };
        self.end_text();
        self.collapser.step(Event::Close {
            trim: open.trim,
            opened: open.item,
        });
        self.room.reserved = self.room.reserved.saturating_sub(1);
        let close = self.push_item(open.kind.close_item_kind(), node, ItemFlags::NONE);
        if close.is_none() {
            debug_assert!(false, "a close has its item reserved");
        }
        // The ruby container's next annotation flags this close.
        if open.kind == ContainerKind::Annotation
            && let Some(ruby) = open
                .outer
                .and_then(|outer| self.scratch.open.get_mut(outer))
            && ruby.kind == ContainerKind::Ruby
        {
            ruby.annotation_close = close;
        }
        if open.kind == ContainerKind::Annotation {
            self.ruby.annotations = self.ruby.annotations.saturating_sub(1);
        }
        if open.kind == ContainerKind::Ruby {
            self.ruby.depth -= 1;
        }
        self.end_leaf(node);
    }

    /// Returns the current text node if its key is `key`.
    pub(super) fn text_node_keyed(&self, key: NodeKey) -> Option<NodeId> {
        self.text_node
            .node
            .filter(|&node| self.content.nodes.key(node) == key)
    }

    /// Ends the current text node, if any, so it takes no more text.
    pub(super) fn end_text(&mut self) {
        self.end_math_text();
        if let Some(node) = self.text_node.node.take() {
            self.end_leaf(node);
        }
        self.text_node.item = None;
    }

    /// Ends the source text node `math-auto` is measuring, writing a held
    /// character as the whole node.
    pub(super) fn end_math_text(&mut self) {
        if let MathText::Held(key, ch) = mem::replace(&mut self.text_node.math, MathText::None) {
            self.split_first_letter(key, ch.encode_utf8(&mut [0; 4]), true);
        }
        self.text_node.math = MathText::None;
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
    /// Each stack entry caches it, so dropped containers cost nothing to
    /// look past.
    pub(super) fn container(&self) -> NodeId {
        self.scratch
            .open
            .last()
            .map_or(NodeId::BLOCK, |open| open.container)
    }

    /// Appends a node of `kind` with facts `lowered`. Returns `None` if the
    /// node table is full.
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
        // Box queries by key find it.
        if kind.is_inline_box() || matches!(kind, NodeKind::Annotation | NodeKind::Ruby) {
            (self.content.keyed_boxes_mut()).push_unsorted(KeyedBox { key, node: id });
        }
        if kind.is_inline_box() {
            self.content.flags.insert(ContentFlags::INLINE_BOXES);
            // The one box that keeps `initial-letter` is the initial letter.
            let Content { block, facts, .. } = &mut *self.content;
            if block.initial_letter.is_none()
                && facts.box_facts(own.box_).has(BoxFlags::INITIAL_LETTER)
            {
                block.initial_letter = Some(id);
            }
        }
        self.content.record(|map| map.open_node());
        self.text_node.continues = None;
        Some(id)
    }

    /// Opens a container: pushes its node, opening item and stack entry, and
    /// reserves its close item.
    ///
    /// An `anonymous` ruby container closes with its annotation. Returns the
    /// node, or `None` if it does not fit.
    pub(super) fn push_container(
        &mut self,
        container: ContainerKind,
        kind: NodeKind,
        key: NodeKey,
        lowered: LoweredNode,
        trim: WhiteSpaceTrim,
        anonymous: bool,
    ) -> Option<NodeId> {
        self.collapser.step(Event::Open { trim });
        let node = self.push_node(kind, lowered, key)?;
        // Counted before the opening item, so an annotation's is flagged.
        if container == ContainerKind::Annotation {
            self.ruby.annotations += 1;
        } else if container == ContainerKind::Ruby {
            self.ruby.depth += 1;
        }
        let item = self.push_item(container.open_item_kind(), node, ItemFlags::NONE);
        self.room.reserved += 1;
        self.push_open(Open {
            node: Some(node),
            kind: container,
            item: item.unwrap_or(self.content.items.next_id()),
            trim,
            container: node,
            transforms: lowered.transforms,
            outer: None,
            anonymous,
            annotation_close: None,
        });
        if container == ContainerKind::Annotation && item.is_some() {
            self.flag_annotation_follows();
        }
        Some(node)
    }

    /// Flags `ANNOTATION_FOLLOWS` on the ruby container's opening item and
    /// on the close of its previous annotation, for the annotation that just
    /// opened.
    ///
    /// Everything inside the ruby container was closed first, so the
    /// annotation's outer link names it.
    fn flag_annotation_follows(&mut self) {
        let Some(ruby) = self.scratch.open.last().and_then(|open| open.outer) else {
            return;
        };
        let Some(container) = self.scratch.open.get(ruby).copied() else {
            return;
        };
        if container.kind != ContainerKind::Ruby || container.node.is_none() {
            return;
        }
        for id in [Some(container.item), container.annotation_close]
            .into_iter()
            .flatten()
        {
            if let Some(item) = self.content.items.get_mut(id) {
                item.flags.insert(ItemFlags::ANNOTATION_FOLLOWS);
            }
        }
    }

    /// Writes a leaf node with its one item of kind `item`.
    ///
    /// `write` fills the item, if it fits, with its text and extras row.
    /// Returns whether the node fits; if not, nothing is written.
    pub(super) fn push_leaf(
        &mut self,
        kind: NodeKind,
        item: ItemKind,
        lowered: LoweredNode,
        key: NodeKey,
        write: impl FnOnce(&mut Self, ItemId),
    ) -> bool {
        let Some(node) = self.push_node(kind, lowered, key) else {
            return false;
        };
        if let Some(item) = self.push_item(item, node, ItemFlags::NONE) {
            write(self, item);
        }
        self.end_leaf(node);
        true
    }

    /// Appends an empty item at the end of the text. Only a close item may
    /// use the reserved room. Returns `None` if the item doesn't fit.
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
            self.text_node.item = None;
        }
        // Text items don't count: a text item may stay empty, and written
        // text is checked directly.
        if !matches!(
            kind,
            ItemKind::Open | ItemKind::Float | ItemKind::Absolute | ItemKind::Text
        ) {
            self.letters.at_start = false;
        }
        let flags = if self.ruby.annotations > 0 {
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
        self.content.nodes.len() < self.room.limits.nodes
    }

    pub(super) fn has_item_room(&self, count: usize) -> bool {
        self.content.items.len() + self.room.reserved + count <= self.room.limits.items
    }

    pub(super) fn has_text_room(&self, bytes: usize) -> bool {
        !self.room.full && self.content.text.len() + bytes <= self.room.limits.text
    }
}
