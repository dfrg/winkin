//! The writer's `::first-letter` calls: arming it, splitting text at the
//! letter, and writing the letter's box.

use super::{ContainerKind, ContentWriter, FirstLetter, InitialLetterUse, MathText, NodeFacts};
use crate::stages::content::memo::StyleKey;
use crate::stages::content::{ContentFlags, NodeId, NodeKey, NodeKind};
use crate::style::{
    BidiGroup, ComputedStyle, Direction, FirstLineVariant, UnicodeBidi, WhiteSpaceTrim,
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
        if matches!(self.text_node.math, MathText::Held(..)) {
            self.end_text();
        }
        if !matches!(
            self.letters.first_letter,
            FirstLetter::Unarmed | FirstLetter::Armed { .. }
        ) {
            return;
        }
        // The styles are keyed now, but lowered and noted in the flags only
        // once the box is written, so a first letter that never comes
        // restyles nothing. Its `initial-letter` and `vertical-align` are
        // kept as given, and settled when the box is written, once it is
        // known whether it is the block's initial letter.
        let (own, first_line) = self.key_styles(InitialLetterUse::Unset, style, first_line);
        self.letters.first_letter = FirstLetter::Armed {
            key,
            style: own,
            first_line,
        };
        if self.letters.first_letter.is_math_auto() {
            self.content.flags.insert(ContentFlags::MATH_AUTO);
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
    pub(super) fn write_first_letter(
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
            Some(_) => self.text_node.source,
            None => 0,
        };
        self.end_text();
        self.text_node.continues = Some((text_key, base));
        if !self.has_node_room() || !self.has_item_room(2) {
            self.text_node_text(text_key, text, single);
            return None;
        }
        // Where it sets `initial-letter`, the box is the block's initial
        // letter where no text comes before it, after empty boxes too, as in
        // Chrome. After kept white space outside it, or inside a box that is
        // one already, the value is `normal`, and its `vertical-align`
        // applies.
        let first = self.content.text.is_empty() && !self.letters.initial_letter;
        let letter = match style.line.initial_letter.is_set() {
            false => InitialLetterUse::Unset,
            true if first => InitialLetterUse::Kept,
            true => InitialLetterUse::Normal,
        };
        self.letters.initial_letter |= letter == InitialLetterUse::Kept;
        // Its `unicode-bidi` is `normal` and its `direction` its parent's.
        // Blink's cascade drops both for the pseudo, so it opens no bidi
        // controls and has its edges and seams where its parent's would be.
        let parent = self.container();
        let used = |mut style: StyleKey, direction: Direction| {
            letter.apply_key(&mut style);
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
        let opened = self.push_container(
            ContainerKind::Box,
            NodeKind::FirstLetter,
            key,
            lowered,
            WhiteSpaceTrim::NONE,
            false,
        );
        let Some(node) = opened else {
            self.text_node_text(text_key, text, single);
            return None;
        };
        // The letter is the box's own text. In the offset map it is the
        // caller's text node's, which counts on after the box.
        self.text_node.node = Some(node);
        self.text_node.item = None;
        self.text_node.source = base;
        self.content.record(|map| map.letter_from(text_key));
        self.transform_text(node, text, single);
        self.pop();
        self.text_node.continues = Some((text_key, self.text_node.source));
        Some(node)
    }

    /// Stops looking for the first letter, because the block holds
    /// something the letter would have had to come before.
    ///
    /// A box holding the punctuation an earlier text ended with is made
    /// plain. It takes the facts of a child that sets nothing in the box
    /// around it, as that punctuation would have.
    pub(super) fn first_letter_ends(&mut self) {
        if let FirstLetter::Punctuation { node } = self.letters.first_letter {
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
        self.letters.first_letter = FirstLetter::Done;
    }
}
