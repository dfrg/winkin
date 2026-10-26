//! `::first-letter`: arming it, writing the letter's box, and ending the
//! search.

use super::{ContainerKind, ContentWriter, FirstLetter, InitialLetterUse, MathText, NodeFacts};
use crate::stages::content::memo::StyleKey;
use crate::stages::content::{ContentFlags, NodeId, NodeKey, NodeKind};
use crate::style::{
    BidiGroup, ComputedStyle, Direction, FirstLineVariant, UnicodeBidi, WhiteSpaceTrim,
};

impl ContentWriter<'_> {
    /// Requests a `::first-letter` box for `key`.
    ///
    /// The next text holding a letter is split after its first letter,
    /// which is written in the box, styled `style` and, on the first line,
    /// `first_line` (`None` for `style`).
    ///
    /// A repeated call before the letter is found replaces the styles. A
    /// call after the letter is found or ruled out is ignored.
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
        // Styles are keyed now but lowered only when the box is written, so
        // a letter that never comes sets no flags. `initial-letter` and
        // `vertical-align` are settled then too, once it is known whether
        // the box is the initial letter.
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

    /// Writes the first letter `text` in a `FirstLetter` box for `key` and
    /// returns the box's node.
    ///
    /// The box holds its text between its opening and closing items, so the
    /// letter is drawn and queried as the pseudo-element. The text node
    /// `text_key` ends first; text after the box is a new node for it. If the
    /// box doesn't fit, `text` is written as `text_key`'s and this returns
    /// `None`.
    pub(super) fn write_first_letter(
        &mut self,
        key: NodeKey,
        style: StyleKey,
        first_line: Option<StyleKey>,
        text_key: NodeKey,
        text: &str,
        single: bool,
    ) -> Option<NodeId> {
        // The letter's source offset in the text node, after any of it
        // already written. If the box doesn't fit, a new node continues
        // from there.
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
        // A box setting `initial-letter` is the initial letter when no text
        // precedes it, even after empty boxes. After kept white space, or
        // inside a box that already is one, it is `normal` and its
        // `vertical-align` applies.
        let first = self.content.text.is_empty() && !self.letters.initial_letter;
        let letter = match style.line.initial_letter.is_set() {
            false => InitialLetterUse::Unset,
            true if first => InitialLetterUse::Kept,
            true => InitialLetterUse::Normal,
        };
        self.letters.initial_letter |= letter == InitialLetterUse::Kept;
        // `unicode-bidi` is `normal` and `direction` the parent's, as Blink's
        // cascade drops both for the pseudo, so it opens no bidi controls.
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
        // The letter is the box's text. The offset map counts it in the
        // source text node, which continues after the box.
        self.text_node.node = Some(node);
        self.text_node.item = None;
        self.text_node.source = base;
        self.content.record(|map| map.letter_from(text_key));
        self.transform_text(node, text, single);
        self.pop();
        self.text_node.continues = Some((text_key, self.text_node.source));
        Some(node)
    }

    /// Stops looking for the first letter.
    ///
    /// A box holding only leading punctuation is made plain: it takes the
    /// facts of a child that sets nothing.
    pub(super) fn first_letter_ends(&mut self) {
        if let FirstLetter::Punctuation { node } = self.letters.first_letter {
            let parent = self.content.nodes.parent(node);
            // Such a child has the parent's text facts, since text
            // properties inherit, and a box keeping only the parent's
            // direction. The parent already set the first-line flags.
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
            // A plain box is not the initial letter.
            let block = &mut self.content.block;
            if block.initial_letter == Some(node) {
                block.initial_letter = None;
            }
        }
        self.letters.first_letter = FirstLetter::Done;
    }
}
