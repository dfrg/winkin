//! The debug build's checks that a finished content keeps its invariants.

use core::cell::Cell;

use super::{ContentWriter, OBJECT};
use crate::data::{Id, TextOffset};
use crate::stages::content::{ItemId, ItemKind};

impl ContentWriter<'_> {
    /// Checks the content's invariants in debug builds, in time linear in
    /// the nodes and items.
    ///
    /// - The items tile the text.
    /// - Atomic inlines and breaks hold their one character.
    /// - The nodes' item ranges nest as the nodes do, and each item is
    ///   inside its own node's range.
    /// - The offset map and the first line's text map stay in order.
    pub(super) fn debug_check(&self) {
        let content = &*self.content;
        let mut at = 0;
        for (id, item) in content.items.iter() {
            let (start, end) = (item.start.get(), item.end.get());
            debug_assert!(start <= end, "{id:?} runs backwards");
            if item.kind.has_text() {
                debug_assert_eq!(
                    start, at,
                    "{id:?} does not start where the text before it ends"
                );
                at = end;
            } else {
                debug_assert_eq!(
                    (start, end),
                    (at, at),
                    "{id:?} holds text or is out of place"
                );
            }
            let text = content.text.get(start..end);
            match item.kind {
                ItemKind::Atomic => debug_assert_eq!(text, Some(OBJECT), "{id:?}"),
                ItemKind::Break => debug_assert_eq!(text, Some("\n"), "{id:?}"),
                _ => debug_assert!(text.is_some(), "{id:?} does not fall on boundaries"),
            }
        }
        debug_assert_eq!(
            at,
            content.text.len(),
            "the items do not reach the text's end"
        );
        let nodes = &content.nodes;
        let mut previous = ItemId::new(0);
        for (node, row) in nodes.nodes.iter() {
            let (first, end) = (row.first_item, row.end);
            debug_assert!(first <= end, "{node:?}'s items run backwards");
            debug_assert!(previous <= first, "{node:?} is out of pre-order");
            previous = first;
            let parent = nodes.items(row.parent);
            debug_assert!(
                parent.start <= first && end <= parent.end,
                "{node:?}'s items are not inside its parent's"
            );
        }
        for (id, item) in content.items.iter() {
            let own = nodes.items(item.node);
            let (first, end) = (own.start, own.end);
            debug_assert!(
                first <= id && id < end,
                "{id:?} is outside its node's items"
            );
        }
        content
            .extras()
            .map
            .debug_check(content.text.len(), content.nodes.len());
        // The first line's text maps each of the text's character
        // boundaries it reaches to one of its own, in order.
        if let Some(source) = content.first_line_source() {
            let mut last = 0;
            let after = Cell::new(0);
            for at in (0..=content.text.len()).filter(|&at| content.text.is_char_boundary(at)) {
                let there = source.offset_stepped(TextOffset::new(at), &after).get();
                if there > source.text.len() {
                    break;
                }
                debug_assert!(there >= last, "the first line's map runs backwards at {at}");
                debug_assert!(
                    source.text.is_char_boundary(there),
                    "the first line's map is off a boundary at {at}"
                );
                last = there;
            }
        }
    }
}
