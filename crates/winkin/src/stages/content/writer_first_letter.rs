//! The writer's `::first-letter` calls: arming it, splitting text at the
//! letter, and writing the letter's box.

use super::collapse::Event;
use super::first_letter::FirstLetterScan;
use super::memo::StyleKey;
use super::{
    ContainerKind, ContentFlags, ContentWriter, FirstLetter, InitialLetterUse, ItemFlags, ItemKind,
    NodeFacts, NodeId, NodeKey, NodeKind, Open,
};
use crate::style::{
    BidiGroup, ComputedStyle, Direction, FirstLineVariant, InitialLetter, TextCase, UnicodeBidi,
    WhiteSpaceTrim,
};

impl ContentWriter<'_> {
    /// Asks for `::first-letter` in a box of its own for `key`.
    ///
    /// The next text that holds a letter is split at the end of its first
    /// letter. The letter is written in the box, set in `style` and, where
    /// the block has `::first-line`, in `first_line` (`None` means its own
    /// style there).
    ///
    /// Asked again before any letter is found, the later styles win. Asked
    /// once the letter is found, or once the block holds something the
    /// letter would have had to come before, the call is ignored.
    pub(crate) fn first_letter(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        if self.math_text.is_some() {
            self.end_text();
        }
        if !matches!(
            self.first_letter,
            FirstLetter::Unarmed | FirstLetter::Armed { .. }
        ) {
            return;
        }
        // The styles are keyed now, but lowered and noted in the flags only
        // once the box is written, so a first letter that never comes
        // restyles nothing. Its `initial-letter` is kept, and settled when
        // the box is written, once it is known whether it is the block's
        // initial letter.
        let letter = match style.line.initial_letter.is_set() {
            true => InitialLetterUse::Kept,
            false => InitialLetterUse::Unset,
        };
        let style = &letter.apply(style);
        let own = self.key(style);
        let first_line = self.first_line.then(|| match first_line {
            Some(first_line) => self.key(&letter.apply(&style.pinned_first_line(first_line))),
            None => own,
        });
        if matches!(own.text.transform.case, TextCase::MathAuto)
            || first_line.is_some_and(|s| matches!(s.text.transform.case, TextCase::MathAuto))
        {
            self.content.flags.insert(ContentFlags::MATH_AUTO);
        }
        self.first_letter = FirstLetter::Armed {
            key,
            style: own,
            first_line,
        };
    }

    /// Writes text from the text node `key` while the first letter is still
    /// to be found. Where the text holds the letter, it splits at the
    /// letter's start and end, and the letter goes in its box.
    pub(super) fn text_with_first_letter(&mut self, key: NodeKey, text: &str, single: bool) {
        // Armed, or else after punctuation an earlier text ended with,
        // whose box is already written.
        let armed = match self.first_letter {
            FirstLetter::Armed {
                key,
                style,
                first_line,
            } => Some((key, style, first_line)),
            FirstLetter::Punctuation { .. } => None,
            FirstLetter::Unarmed | FirstLetter::Done => {
                self.plain_text(key, text, single);
                return;
            }
        };
        // The text is read as its own white space collapses: by its node's
        // rules, or by those of the box it would be written in.
        let holder = self
            .text_node_keyed(key)
            .unwrap_or_else(|| self.container());
        let mode = self.text_facts(holder).collapse;
        match (FirstLetterScan::new(text, mode, armed.is_none()), armed) {
            (FirstLetterScan::Nothing, _) => self.plain_text(key, text, single),
            (FirstLetterScan::Ends, _) => {
                self.first_letter_ends();
                self.plain_text(key, text, single);
            }
            // The punctuation before the letter came from an earlier text,
            // and the box holds only that part, as Chrome's does.
            (FirstLetterScan::Text { found, .. }, None) => {
                if found {
                    self.first_letter = FirstLetter::Done;
                }
                self.plain_text(key, text, single);
            }
            (FirstLetterScan::Text { start, end, found }, Some((letter, style, first_line))) => {
                let (before, rest) = text.split_at(start.min(text.len()));
                let (letter_text, after) = rest.split_at(end.saturating_sub(start).min(rest.len()));
                if !before.is_empty() {
                    self.plain_text(key, before, single);
                }
                let node =
                    self.write_first_letter(letter, style, first_line, key, letter_text, single);
                self.first_letter = match node {
                    Some(node) if !found => FirstLetter::Punctuation { node },
                    _ => FirstLetter::Done,
                };
                if !after.is_empty() {
                    self.plain_text(key, after, single);
                }
            }
        }
    }

    /// Writes `text`, the first letter, in its box, and returns the box's
    /// node.
    ///
    /// The box is a `FirstLetter` node for `key`, in `style` and
    /// `first_line`. It holds its own text between its opening and closing
    /// items, so the letter is drawn as the pseudo-element and answers to
    /// the caller as it.
    ///
    /// The text node `text_key` is ended first, and what follows the box is
    /// a new node of it. Where the box does not fit, the text is written as
    /// `text_key`'s, and this returns `None`.
    fn write_first_letter(
        &mut self,
        key: NodeKey,
        style: StyleKey,
        first_line: Option<StyleKey>,
        text_key: NodeKey,
        text: &str,
        single: bool,
    ) -> Option<NodeId> {
        // Where the letter starts in the caller's text node: after what of
        // it was written before, in this call or earlier ones. Where the box
        // does not fit, the text goes on from there in a node of its own.
        let base = match self.text_node_keyed(text_key) {
            Some(_) => self.source,
            None => 0,
        };
        self.end_text();
        self.continues = Some((text_key, base));
        if !self.has_node_room() || !self.has_item_room(2) {
            self.plain_text(text_key, text, single);
            return None;
        }
        // Where it sets `initial-letter`, the box is the block's initial
        // letter if it may be one. It may not after kept white space, or
        // inside a box that is one; there the value is `normal`.
        let initial_letter = style.line.initial_letter.is_set();
        let kept = initial_letter && self.may_open_initial_letter();
        self.initial_letter |= kept;
        // Its `unicode-bidi` is `normal` and its `direction` its parent's.
        // Blink's cascade drops both for the pseudo, so it opens no bidi
        // controls and has its edges and seams where its parent's would be.
        let parent = self.container();
        let used = |mut style: StyleKey, direction: Direction| {
            if !kept {
                style.line.initial_letter = InitialLetter::NONE;
            }
            style.bidi = BidiGroup {
                direction,
                unicode_bidi: UnicodeBidi::Normal,
            };
            style
        };
        let style = used(style, self.direction(parent, FirstLineVariant::Standard));
        let first_line = first_line
            .map(|first| used(first, self.direction(parent, FirstLineVariant::FirstLine)));
        let around = self.container_facts();
        let lowered = self.lower_both(
            &style,
            first_line.as_ref(),
            NodeKind::FirstLetter,
            Some(around),
        );
        self.collapser.step(Event::Open {
            trim: WhiteSpaceTrim::NONE,
        });
        let Some(node) = self.push_node(NodeKind::FirstLetter, lowered, key) else {
            self.plain_text(text_key, text, single);
            return None;
        };
        let item = self.push_item(ItemKind::Open, node, ItemFlags::NONE);
        self.reserved += 1;
        self.push_open(Open {
            node: Some(node),
            kind: ContainerKind::Box,
            item: item.unwrap_or(self.content.items.next_id()),
            trim: WhiteSpaceTrim::NONE,
            container: node,
            transforms: lowered.transforms,
            outer: None,
            anonymous: false,
        });
        // The letter is the box's own text. In the offset map it is the
        // caller's text node's, which counts on after the box.
        self.text_node = Some(node);
        self.open_item = None;
        self.source = base;
        self.content.record(|map| map.letter_from(text_key));
        self.write_node_text(node, text, single);
        self.pop();
        self.continues = Some((text_key, self.source));
        Some(node)
    }

    /// Stops looking for the first letter, because the block holds
    /// something the letter would have had to come before.
    ///
    /// A box holding the punctuation an earlier text ended with is made
    /// plain. It takes the facts of a child that sets nothing in the box
    /// around it, as that punctuation would have.
    pub(super) fn first_letter_ends(&mut self) {
        if let FirstLetter::Punctuation { node } = self.first_letter {
            let parent = self.content.nodes.parent(node);
            // A child that sets nothing has its parent's text facts, since
            // every text property inherits. Its box keeps nothing of its own
            // but its direction, which is its parent's. The parent already
            // noted whether its first line restyles or reshapes it.
            let mut plain = |variant: FirstLineVariant| NodeFacts {
                text: self.content.nodes.text_facts(parent, variant),
                box_: self.plain_box(self.direction(parent, variant)),
            };
            let own = plain(FirstLineVariant::Standard);
            let first_line = plain(FirstLineVariant::FirstLine);
            let nodes = &mut self.content.nodes;
            if let Some(slot) = nodes.nodes.get_mut(node) {
                (slot.text, slot.box_) = (own.text, own.box_);
                (slot.first_line_text, slot.first_line_box) = (first_line.text, first_line.box_);
            }
            // Made plain, it is no initial letter, and the block has none.
            let block = &mut self.content.block;
            if block.initial_letter == Some(node) {
                block.initial_letter = None;
            }
        }
        self.first_letter = FirstLetter::Done;
    }
}
