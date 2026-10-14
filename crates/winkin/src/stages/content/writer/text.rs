//! Writes text: white space collapsing, transforms, masks, forced breaks,
//! the offset map and the first-line text.
//!
//! A text call passes down a ladder, each rung adding one thing:
//! - `text`: the builder's call;
//! - `hold_math_text`: holds back a lone character `math-auto` may map
//!   until its source node continues or ends;
//! - `split_first_letter`: splits at the `::first-letter` box while one is
//!   sought (`find_first_letter`);
//! - `text_node_text`: finds or makes the text node;
//! - `transform_text`: picks the node's transforms;
//! - `collapse_text`: collapses white space and writes the pieces between;
//! - `write_piece`: writes one transformed piece, with its offset map units
//!   and first-line copy.

use alloc::boxed::Box;
use alloc::string::String;
use core::mem;

use icu_segmenter::GraphemeClusterSegmenter;

use super::{ContentWriter, FirstLetter, MathText, Mirror};
use crate::data::{TextOffset, index_to_u32};
use crate::stages::content::collapse::{Anchor, Effect, Event, Run, ZWSP};
use crate::stages::content::first_letter::FirstLetterScan;
use crate::stages::content::map::Map;
use crate::stages::content::transform::{TextTransformer, Transforms, align_counts};
use crate::stages::content::{
    Content, ContentExtras, ContentFlags, Item, ItemFlags, ItemId, ItemKind, NodeId, NodeKey,
    NodeKind, TextFlags,
};
use crate::style::{TextCombineUpright, WhiteSpaceCollapse};
use crate::unicode::is_mark;
use crate::work;

impl ContentWriter<'_> {
    /// Writes text from text node `key`.
    ///
    /// Text goes through [`hold_math_text`](Self::hold_math_text) only once
    /// some style uses `math-auto`.
    pub(crate) fn text(&mut self, key: NodeKey, text: &str) {
        if !self.content.flags.contains(ContentFlags::MATH_AUTO) {
            self.split_first_letter(key, text, false);
            return;
        }
        self.hold_math_text(key, text);
    }

    /// Writes text from text node `key`, holding back a lone character that
    /// `math-auto` may map until the node continues or ends.
    ///
    /// `math-auto` maps only one-character source nodes, known only when the
    /// node ends. Ending a text node writes a held character
    /// (`end_math_text`), which re-enters the ladder below this rung.
    #[inline(never)]
    fn hold_math_text(&mut self, key: NodeKey, text: &str) {
        match mem::replace(&mut self.text_node.math, MathText::None) {
            MathText::Long(long) if long == key => {
                self.split_first_letter(key, text, false);
                self.text_node.math = MathText::Long(key);
                return;
            }
            MathText::Held(held, ch) if held == key => {
                if text.is_empty() {
                    self.text_node.math = MathText::Held(held, ch);
                    return;
                }
                self.split_first_letter(held, ch.encode_utf8(&mut [0; 4]), false);
                self.split_first_letter(key, text, false);
                self.text_node.math = MathText::Long(key);
                return;
            }
            MathText::Held(held, ch) => {
                self.split_first_letter(held, ch.encode_utf8(&mut [0; 4]), true);
            }
            MathText::Long(_) | MathText::None => {}
        }
        let math =
            self.container_transforms().has_math_auto() || self.letters.first_letter.is_math_auto();
        if math {
            let mut chars = text.chars();
            if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
                if self.text_node_keyed(key).is_none() {
                    self.end_text();
                }
                self.text_node.math = MathText::Held(key, ch);
                return;
            }
        }
        self.split_first_letter(key, text, false);
        if math && !text.is_empty() {
            self.text_node.math = MathText::Long(key);
        }
    }

    /// Writes text from text node `key`, split at the `::first-letter` box
    /// while one is sought.
    ///
    /// `single` means `text` is a whole one-character source node.
    #[inline]
    pub(super) fn split_first_letter(&mut self, key: NodeKey, text: &str, single: bool) {
        match self.letters.first_letter {
            FirstLetter::Armed { .. } | FirstLetter::Punctuation { .. } if !self.room.full => {
                self.find_first_letter(key, text, single);
            }
            _ => self.text_node_text(key, text, single),
        }
    }

    /// Writes text from text node `key` while the first letter is sought,
    /// splitting the letter out into its box if the text holds it.
    fn find_first_letter(&mut self, key: NodeKey, text: &str, single: bool) {
        // `None` when an earlier text's leading punctuation already has its
        // box.
        let armed = match self.letters.first_letter {
            FirstLetter::Armed {
                key,
                style,
                first_line,
            } => Some((key, style, first_line)),
            FirstLetter::Punctuation { .. } => None,
            FirstLetter::Unarmed | FirstLetter::Done => {
                self.text_node_text(key, text, single);
                return;
            }
        };
        // Read with the white space rules of its node, or of the container
        // a new node would go in.
        let holder = self
            .text_node_keyed(key)
            .unwrap_or_else(|| self.container());
        let mode = self.text_facts(holder).collapse;
        match (FirstLetterScan::new(text, mode, armed.is_none()), armed) {
            (FirstLetterScan::Nothing, _) => self.text_node_text(key, text, single),
            (FirstLetterScan::Ends, _) => {
                self.first_letter_ends();
                self.text_node_text(key, text, single);
            }
            // The leading punctuation came from an earlier text; the box
            // holds only that, as Chrome's does.
            (FirstLetterScan::Text { found, .. }, None) => {
                if found {
                    self.letters.first_letter = FirstLetter::Done;
                }
                self.text_node_text(key, text, single);
            }
            (FirstLetterScan::Text { start, end, found }, Some((letter, style, first_line))) => {
                let (before, rest) = text.split_at(start.min(text.len()));
                let (letter_text, after) = rest.split_at(end.saturating_sub(start).min(rest.len()));
                if !before.is_empty() {
                    self.text_node_text(key, before, single);
                }
                let node =
                    self.write_first_letter(letter, style, first_line, key, letter_text, single);
                self.letters.first_letter = match node {
                    Some(node) if !found => FirstLetter::Punctuation { node },
                    _ => FirstLetter::Done,
                };
                if !after.is_empty() {
                    self.text_node_text(key, after, single);
                }
            }
        }
    }

    /// Writes text from text node `key` into the current text node if it is
    /// `key`'s, else into a new one.
    pub(super) fn text_node_text(&mut self, key: NodeKey, text: &str, single: bool) {
        let continues = self.text_node_keyed(key).is_some();
        if self.room.full {
            // Nothing more fits: drop the text, and its node if new.
            self.room.report.drop_bytes(text.len());
            if !continues {
                self.end_text();
                self.room.report.drop_node();
            }
            return;
        }
        let node = match self.text_node.node {
            Some(node) if continues => node,
            _ => {
                self.end_text();
                // The rest of a text node split by the first letter counts
                // source bytes from the letter's end.
                let base = match self.text_node.continues.take() {
                    Some((continued, at)) if continued == key => at,
                    _ => 0,
                };
                let around = self.container_facts();
                // The node and its item both fit, or neither is written.
                let node = if self.has_item_room(1) {
                    self.push_node(NodeKind::Text, around, key)
                } else {
                    None
                };
                let Some(node) = node else {
                    self.room.report.drop_node();
                    self.room.report.drop_bytes(text.len());
                    return;
                };
                self.text_node.node = Some(node);
                self.text_node.source = base;
                node
            }
        };
        self.transform_text(node, text, single);
    }

    /// Writes `text` for the current text node `node`.
    ///
    /// White space follows the node's text facts; transforms come from its
    /// container.
    pub(super) fn transform_text(&mut self, node: NodeId, text: &str, single: bool) {
        let facts = self.text_facts(node);
        let (mode, wraps) = (facts.collapse, facts.has(TextFlags::WRAPS));
        let combined = facts.combine == TextCombineUpright::All;
        self.combined_text_arrives(combined);
        let transforms = self.container_transforms();
        // The first-line transform matters only within the first paragraph.
        let transforms = match self.first_line.mirror {
            Mirror::Done => transforms.without_first_line(),
            Mirror::Waiting | Mirror::Writing => transforms,
        };
        let transforms = if self.content.flags.contains(ContentFlags::MATH_AUTO) {
            transforms.with_node(single, text)
        } else {
            transforms
        };
        self.collapse_text(mode, wraps, transforms, text);
        if combined && !text.is_empty() && self.collapser.is_combining_empty() {
            self.keep_empty_unit(node);
        }
    }

    /// Gives a combined unit whose text all collapsed away a generated
    /// U+200B.
    ///
    /// The unit stays one em wide, even at a line end, as Blink's
    /// `LayoutTextCombine` is an inline block whatever its text.
    #[cold]
    #[inline(never)]
    fn keep_empty_unit(&mut self, node: NodeId) {
        if !self.has_text_room(ZWSP.len_utf8()) {
            self.room.full = true;
            return;
        }
        self.generated_break(node, self.text_node.source);
        self.collapser.step(Event::Content { first: ZWSP });
    }

    /// Writes `text` for the current text node with white space mode
    /// `mode` and `transforms`.
    ///
    /// Pieces between collapsible white space are transformed separately.
    /// No transform makes or removes collapsible white space, and no case
    /// mapping reads context across it, so this equals transforming the
    /// whole text and then collapsing, as Chrome does. Each piece gets the
    /// character before it, for capitalize.
    pub(super) fn collapse_text(
        &mut self,
        mode: WhiteSpaceCollapse,
        wraps: bool,
        transforms: Transforms,
        text: &str,
    ) {
        use WhiteSpaceCollapse as Ws;
        let Some(last) = text.chars().next_back() else {
            return;
        };
        if let Some(mask) = transforms.mask {
            let before = mem::replace(&mut self.text_node.last_char, mask);
            self.write_masked(mask, transforms, before, text);
            return;
        }
        // The next text's `before`, however much of this is kept.
        let mut before = mem::replace(&mut self.text_node.last_char, last);
        // With collapsing spaces and no transform, a lone space between two
        // plain characters is its run's own space, so it is written with
        // the text around it.
        let lone_spaces =
            mode.collapses_spaces() && transforms.own.is_none() && !transforms.first_line_differs();
        let mut rest = text;
        while !rest.is_empty() {
            if self.room.full {
                self.room.report.drop_bytes(rest.len());
                return;
            }
            let plain = plain_len(rest, !mode.keeps_spaces(), lone_spaces);
            // `plain_len` returns a character boundary.
            let (content, tail) = rest.split_at(plain);
            if !content.is_empty() {
                let kept = self.write_piece(content, None, transforms, before);
                if kept < content.len() {
                    self.room
                        .report
                        .drop_bytes(content.len() - kept + tail.len());
                    return;
                }
                before = content.chars().next_back().unwrap_or(before);
                rest = tail;
                continue;
            }
            let Some(ch) = rest.chars().next() else {
                return;
            };
            let len = ch.len_utf8();
            let previous = mem::replace(&mut before, ch);
            rest = rest.get(len..).unwrap_or_default();
            let written = match ch {
                // The CR of a CRLF belongs to the LF's segment break, as
                // HTML normalizes CRLF to LF before CSS sees it.
                '\r' if rest.starts_with('\n') => {
                    self.collapsed(len);
                    true
                }
                // Inside ruby or a combined unit no segment break is kept,
                // since no paragraph ends there. It collapses where only
                // breaks are kept and is a space where spaces are kept, as
                // in Blink (`should_not_preserve_newline` in ruby,
                // `is_text_combine_` in a combined unit).
                '\n' => match (
                    mode,
                    self.ruby.innermost.is_some() || self.collapser.is_combining(),
                ) {
                    (Ws::Discard, _) => {
                        self.collapsed(len);
                        true
                    }
                    (Ws::Collapse, _) | (Ws::PreserveBreaks, true) => {
                        self.white_space(true, wraps, len);
                        true
                    }
                    // A space: with `preserve-spaces`, and in ruby or a
                    // combined unit where spaces are kept.
                    (Ws::PreserveSpaces, _) | (Ws::Preserve | Ws::BreakSpaces, true) => {
                        self.write_piece(" ", Some(len), Transforms::NONE, ch) > 0
                    }
                    // A kept segment break: `preserve`, `break-spaces` and
                    // `preserve-breaks`.
                    (Ws::Preserve | Ws::BreakSpaces | Ws::PreserveBreaks, false) => {
                        self.write_forced_break("\n", true)
                    }
                },
                // A space or tab (here only if it collapses or is
                // discarded), or a lone CR, which CSS Text 3 treats as a
                // space in all respects.
                ' ' | '\t' | '\r' => match mode {
                    Ws::Discard => {
                        self.collapsed(len);
                        true
                    }
                    Ws::Collapse | Ws::PreserveBreaks => {
                        self.white_space(false, wraps, len);
                        true
                    }
                    Ws::Preserve | Ws::BreakSpaces | Ws::PreserveSpaces => {
                        self.write_piece(" ", Some(len), transforms, previous) > 0
                    }
                },
                // VT, FF, NEL, U+2028 and U+2029 force a break in every mode
                // (CSS Text 3, section 5.1). In ruby it is a space, as a
                // `<br>` there is.
                _ if self.ruby.innermost.is_some() => {
                    let kept = self.write_piece(" ", Some(len), Transforms::NONE, ch) > 0;
                    if kept {
                        self.collapser.step(Event::ForcedBreak);
                    }
                    kept
                }
                _ => self.write_forced_break(ch.encode_utf8(&mut [0; 4]), false),
            };
            if !written {
                self.room.report.drop_bytes(ch.len_utf8() + rest.len());
                return;
            }
        }
    }

    /// Writes `content`, which has no collapsible white space, for the
    /// current text node, and mirrors it to the first-line text.
    ///
    /// `before` is the source character before it. `stands` is the source
    /// byte count when `content` replaces source text (a space for a CR or
    /// segment break). Returns the bytes of `content` kept: all unless the
    /// text or items ran out.
    fn write_piece(
        &mut self,
        content: &str,
        stands: Option<usize>,
        transforms: Transforms,
        before: char,
    ) -> usize {
        let Some(first) = content.chars().next() else {
            return 0;
        };
        self.content_arrives(first, content.chars().all(is_document_white_space));
        let Some(item) = self.open_item() else {
            return 0;
        };
        if transforms.first_line_differs() && self.first_line.mirror == Mirror::Waiting {
            // The first text with a differing first-line transform starts
            // the mirror with the text so far.
            let Content { text, extras, .. } = &mut *self.content;
            let extras = extras.get_or_insert_with(Box::default);
            extras.first_line_source.start(text);
            self.content.flags.insert(ContentFlags::TRANSFORM_SIDE_TEXT);
            self.first_line.mirror = Mirror::Writing;
        }
        let mark = TextOffset::new(self.content.text.len());
        let room = self.room.limits.text.saturating_sub(mark.get());
        let kept = match transforms.own {
            None => {
                let mut kept = content.len().min(room);
                while !content.is_char_boundary(kept) {
                    kept -= 1;
                }
                let (fits, _) = content.split_at(kept);
                self.content.text.push_str(fits);
                self.extend(item);
                if stands.is_none() {
                    self.written_as_given(kept, mark);
                }
                kept
            }
            // Room for the largest output: write it whole, in context, then
            // map each source character to its output.
            Some(transformer) if content.len().saturating_mul(transformer.max_growth()) <= room => {
                transformer.write(
                    content,
                    before,
                    &mut self.scratch.words,
                    &mut self.content.text,
                );
                self.extend(item);
                if stands.is_none() {
                    self.transformed(mark, content, transformer, before);
                }
                content.len()
            }
            // Near the limit, transform one character at a time and cut
            // after the last one whose output fits.
            Some(transformer) => {
                let mut kept = 0;
                let mut previous = before;
                let mut one = [0u8; 4];
                for ch in content.chars() {
                    let back_to = self.content.text.len();
                    let alone = ch.encode_utf8(&mut one);
                    transformer.write(
                        alone,
                        previous,
                        &mut self.scratch.words,
                        &mut self.content.text,
                    );
                    if self.content.text.len() > self.room.limits.text {
                        self.content.text.truncate(back_to);
                        break;
                    }
                    kept += ch.len_utf8();
                    previous = ch;
                }
                self.extend(item);
                if stands.is_none() {
                    let written = content.get(..kept).unwrap_or_default();
                    self.transformed(mark, written, transformer, before);
                }
                kept
            }
        };
        if let Some(len) = stands
            && kept > 0
        {
            self.character_stands(len, mark);
        }
        let most = content.len().saturating_mul(transforms.max_growth());
        self.mirror(most, |writer| {
            writer.mirror_content(content, kept, mark, transforms, before);
        });
        if kept < content.len() {
            self.room.full = true;
        }
        kept
    }

    /// Writes `text` masked by `-webkit-text-security`: one `mask` per
    /// grapheme of the transformed text.
    ///
    /// Chrome masks before white space processing (`LayoutText::SecureText`),
    /// so spaces, tabs and segment breaks are masked too; none collapses or
    /// breaks a line. Each source grapheme is one offset map unit, so a mask
    /// is one caret stop. `before` is the character before `text`, for
    /// capitalize.
    fn write_masked(&mut self, mask: char, transforms: Transforms, mut before: char, text: &str) {
        self.content_arrives(mask, false);
        let mut buffer = [0; 4];
        let mask: &str = mask.encode_utf8(&mut buffer);
        let Some(item) = self.open_item() else {
            self.room.report.drop_bytes(text.len());
            return;
        };
        let mut boundaries = GraphemeClusterSegmenter::new().segment_str(text);
        // The segmenter yields 0 first.
        let mut start = boundaries.next().unwrap_or(0);
        for end in boundaries {
            work::step();
            let grapheme = text.get(start..end).unwrap_or_default();
            let count = match transforms.own {
                Some(transformer) => self.transformed_graphemes(transformer, grapheme, before),
                None => 1,
            };
            if !self.has_text_room(count.saturating_mul(mask.len())) {
                self.room.full = true;
                self.room.report.drop_bytes(text.len() - start);
                return;
            }
            let at = TextOffset::new(self.content.text.len());
            for _ in 0..count {
                self.append(item, mask);
            }
            let one_char = grapheme.chars().nth(1).is_none();
            if one_char && count == 1 && grapheme.len() == mask.len() {
                self.written_as_given(grapheme.len(), at);
            } else {
                let end = self
                    .text_node
                    .source
                    .saturating_add(index_to_u32(grapheme.len()));
                self.content
                    .record(|map| map.variable(self.text_node.source..end, at));
                self.text_node.source = end;
            }
            before = grapheme.chars().next_back().unwrap_or(before);
            start = end;
        }
    }

    /// Returns how many graphemes `transformer` makes of `grapheme`.
    ///
    /// It writes the output past the text's end and truncates it again, so
    /// it needs no buffer. If the output might not fit, it returns 1.
    fn transformed_graphemes(
        &mut self,
        transformer: TextTransformer,
        grapheme: &str,
        before: char,
    ) -> usize {
        let mark = self.content.text.len();
        if !self.has_text_room(grapheme.len().saturating_mul(transformer.max_growth())) {
            return 1;
        }
        transformer.write(
            grapheme,
            before,
            &mut self.scratch.words,
            &mut self.content.text,
        );
        let written = self.content.text.get(mark..).unwrap_or_default();
        // The segmenter yields 0 first, and only that for empty text.
        let count = GraphemeClusterSegmenter::new()
            .segment_str(written)
            .count()
            .saturating_sub(1);
        self.content.text.truncate(mark);
        count
    }

    /// Steps the collapser over one collapsible white space character of
    /// `len` source bytes.
    fn white_space(&mut self, segment_break: bool, wraps: bool, len: usize) {
        let anchor = self.anchor();
        let at = TextOffset::new(self.content.text.len());
        let source = self.text_node.source..self.text_node.source.saturating_add(index_to_u32(len));
        self.text_node.source = source.end;
        match self.collapser.step(Event::Space {
            segment_break,
            wraps,
            anchor,
        }) {
            Effect::Began => {
                // Push the run's item now if the node has none, so its id is
                // the anchor's.
                if self.text_node.item.is_none() {
                    let item = self.open_item();
                    debug_assert_eq!(item, anchor.map(|anchor| anchor.item));
                }
                self.text_node.run_unit = self.content.record(|map| map.run(source, at)).flatten();
            }
            Effect::GenerateBreak => {
                if !self.has_text_room(ZWSP.len_utf8()) {
                    self.room.full = true;
                } else if let Some(node) = self.text_node.node {
                    self.generated_break(node, source.start);
                }
                let at = TextOffset::new(self.content.text.len());
                self.content.record(|map| map.collapsed(source, at));
            }
            Effect::None | Effect::WriteSpace(_) => {
                self.content.record(|map| map.collapsed(source, at));
            }
        }
    }

    /// Records `len` source bytes collapsed or discarded, with nothing
    /// written.
    fn collapsed(&mut self, len: usize) {
        let at = TextOffset::new(self.content.text.len());
        let source = self.text_node.source..self.text_node.source.saturating_add(index_to_u32(len));
        self.text_node.source = source.end;
        self.content.record(|map| map.collapsed(source, at));
    }

    /// Records `len` source bytes written unchanged at `at` (see
    /// [`is_joined`](Self::is_joined)).
    fn written_as_given(&mut self, len: usize, at: TextOffset) {
        let joins = self.is_joined(at);
        self.content
            .record(|map| map.identity(self.text_node.source, len, at, joins));
        self.text_node.source = self.text_node.source.saturating_add(index_to_u32(len));
    }

    /// Whether text written unchanged at `at` joins the offset map unit
    /// before it.
    ///
    /// It doesn't when the block trims kept white space and the text follows
    /// a kept segment break, where the trim may cut.
    fn is_joined(&self, at: TextOffset) -> bool {
        !(self.trim.ends
            && self
                .content
                .text
                .get(..at.get())
                .is_some_and(|before| before.ends_with('\n')))
    }

    /// Returns where a run beginning now would put its space, or `None` if
    /// its item would not fit.
    fn anchor(&self) -> Option<Anchor> {
        let item = match self.text_node.item {
            Some(item) => item,
            None if self.has_item_room(1) => self.content.items.next_id(),
            None => return None,
        };
        Some(Anchor {
            at: TextOffset::new(self.content.text.len()),
            item,
            before: self.content.text.chars().next_back(),
        })
    }

    /// Writes the forced break `written` unchanged in the text node's item.
    /// Returns false if it does not fit.
    ///
    /// The break is a kept `\n` (`kept_segment_break`) or VT, FF, NEL,
    /// U+2028 or U+2029. It removes collapsible white space on both sides
    /// and ends the first paragraph, unless the leading trim is still
    /// pending.
    ///
    /// The leading trim treats a kept segment break as white space and may
    /// cut after it, so it starts an offset map unit when the block trims.
    /// The trim treats the other characters as content.
    fn write_forced_break(&mut self, written: &str, kept_segment_break: bool) -> bool {
        if !self.has_text_room(written.len()) {
            self.room.full = true;
            return false;
        }
        self.collapser.step(Event::ForcedBreak);
        if !kept_segment_break {
            self.lead_trim_ends();
        }
        let Some(item) = self.open_item() else {
            return false;
        };
        let at = TextOffset::new(self.content.text.len());
        self.append(item, written);
        let joins = if kept_segment_break {
            !self.trim.ends
        } else {
            self.is_joined(at)
        };
        self.content
            .record(|map| map.identity(self.text_node.source, written.len(), at, joins));
        self.text_node.source = self
            .text_node
            .source
            .saturating_add(index_to_u32(written.len()));
        if self.trim.lead.is_some() {
            // The leading trim will remove this break too, so the first
            // paragraph has not ended.
            self.trim.lead = Some(TextOffset::new(self.content.text.len()));
        } else {
            self.first_line.mirror = Mirror::Done;
        }
        true
    }

    /// Records offset map units for source text `src`, transformed into the
    /// text from `mark` to the end.
    ///
    /// A character mapped to one of the same length is recorded as
    /// unchanged; one of another length gets a `Variable` unit. If context
    /// changed the mapping (a Greek accent dropped under a capital), units
    /// fall back to base characters with their marks, then to one unit for
    /// the piece.
    fn transformed(
        &mut self,
        mark: TextOffset,
        src: &str,
        transformer: TextTransformer,
        before: char,
    ) {
        let source = self.text_node.source;
        let end = source.saturating_add(index_to_u32(src.len()));
        let joins = self.is_joined(mark);
        self.text_node.source = end;
        let Content { text, extras, .. } = &mut *self.content;
        let Some(map) = extras.as_deref_mut().and_then(ContentExtras::recorded_map) else {
            return;
        };
        let written = text.get(mark.get()..).unwrap_or_default();
        // ASCII mapped to ASCII of the same length maps one to one.
        if src.is_ascii() && written.is_ascii() && src.len() == written.len() {
            map.identity(source, src.len(), mark, joins);
            return;
        }
        // Align each source character with its output alone; if that
        // doesn't add up to the output in context, undo these units.
        let own = Transforms::new(Some(transformer), None);
        let words = &mut self.scratch.words;
        let back = map.mark();
        let whole = align_counts(
            written,
            src,
            |each| own.counts(src, before, words, &mut |ch, count, _| each(ch, count, 1)),
            |made| {
                let at = TextOffset::new(mark.get() + made.ours.start.get());
                let from = source.saturating_add(index_to_u32(made.theirs.start.get()));
                if made.is_aligned() {
                    map.identity(
                        from,
                        made.ours.end.get() - made.ours.start.get(),
                        at,
                        joins || at != mark,
                    );
                } else {
                    let to = source.saturating_add(index_to_u32(made.theirs.end.get()));
                    map.variable(from..to, at);
                }
            },
        );
        if !whole {
            map.truncate(back);
            if !transformed_clusters(map, written, src, transformer, source, mark) {
                map.truncate(back);
                map.variable(source..end, mark);
            }
        }
    }

    /// Records the offset map unit for the text from `at` to the end, which
    /// replaces `len` source bytes: unchanged if the lengths match, else
    /// `Variable`.
    fn character_stands(&mut self, len: usize, at: TextOffset) {
        let written = self.content.text.len() - at.get();
        if written == len {
            self.written_as_given(len, at);
        } else {
            let end = self.text_node.source.saturating_add(index_to_u32(len));
            self.content
                .record(|map| map.variable(self.text_node.source..end, at));
            self.text_node.source = end;
        }
    }

    /// Runs `write`, which adds up to `bytes` to the first-line text, while
    /// the mirror is writing.
    ///
    /// The first-line text is bounded by the text limit. If `bytes` would
    /// pass it, mirroring stops and the first line draws what it has.
    fn mirror(&mut self, bytes: usize, write: impl FnOnce(&mut Self)) {
        if self.first_line.mirror != Mirror::Writing {
            return;
        }
        if self.mirror_room(bytes) {
            write(self);
        } else {
            self.first_line.mirror = Mirror::Done;
        }
    }

    /// Whether the first-line text has room for `bytes` more within the
    /// text limit.
    fn mirror_room(&self, bytes: usize) -> bool {
        let held = self
            .content
            .first_line_source()
            .map_or(0, |source| source.text.len());
        held.saturating_add(bytes) <= self.room.limits.text
    }

    /// Mirrors to the first-line text what
    /// [`write_piece`](Self::write_piece) wrote from `mark`, of which `kept`
    /// bytes of source `content` were kept.
    ///
    /// With a differing first-line transform, it writes that transform of
    /// the source instead. A piece cut at the text limit is copied as is.
    fn mirror_content(
        &mut self,
        content: &str,
        kept: usize,
        mark: TextOffset,
        transforms: Transforms,
        before: char,
    ) {
        let Content { text, extras, .. } = &mut *self.content;
        let first_line_source = &mut extras.get_or_insert_with(Box::default).first_line_source;
        let written = text.get(mark.get()..).unwrap_or_default();
        if transforms.first_line_differs() && kept == content.len() {
            first_line_source.push_transformed(
                mark,
                written,
                content,
                before,
                transforms,
                &mut self.scratch.words,
            );
        } else {
            first_line_source.push_same(mark, written);
        }
    }

    /// Settles white space as content starting with `first` arrives.
    ///
    /// A pending run writes its space. Unless the content is kept white
    /// space (`white`), the leading trim cuts and an unarmed first letter is
    /// ruled out.
    pub(super) fn content_arrives(&mut self, first: char, white: bool) {
        if let Effect::WriteSpace(run) = self.collapser.step(Event::Content { first }) {
            self.write_space(run);
        }
        if !white {
            self.lead_trim_ends();
            // A first letter requested after this would not be first.
            if self.letters.first_letter == FirstLetter::Unarmed {
                self.letters.first_letter = FirstLetter::Done;
            }
        }
    }

    /// Tells the collapser whether the following text is `combined`. A
    /// starting unit settles the run before it, as an atomic inline does.
    pub(super) fn combined_text_arrives(&mut self, combined: bool) {
        if let Effect::WriteSpace(run) = self.collapser.step(Event::Combined(combined)) {
            self.write_space(run);
        }
    }

    /// Inserts the space `run` owes at the run's start, shifting later items.
    fn write_space(&mut self, run: Run) {
        let unit = self.text_node.run_unit.take();
        if !self.has_text_room(1) {
            self.room.report.drop_bytes(1);
            self.room.full = true;
            return;
        }
        let at = run.at.get();
        let Content { text, items, .. } = &mut *self.content;
        // The run's item grows by the space; every later item shifts.
        let end = items.next_id();
        let Some((anchor, after)) = items
            .get_slice_mut(run.item..end)
            .and_then(<[Item]>::split_first_mut)
        else {
            debug_assert!(false, "a run's item exists");
            return;
        };
        if at == text.len() {
            text.push(' ');
        } else if text.is_char_boundary(at) {
            text.insert(at, ' ');
        } else {
            debug_assert!(false, "a run begins on a boundary");
            return;
        }
        let one = |offset: TextOffset| TextOffset::new(offset.get() + 1);
        work::step();
        anchor.end = one(anchor.end);
        for item in after {
            work::step();
            item.start = one(item.start);
            item.end = one(item.end);
        }
        if let Some(unit) = unit {
            self.content.record(|map| map.space_written(unit));
        }
        self.mirror(1, |writer| {
            writer.content.first_line_source_mut().insert_space(run.at);
        });
    }

    /// Ends the block's leading trim, removing the text through its last
    /// kept segment break.
    pub(super) fn lead_trim_ends(&mut self) {
        let Some(cut) = self.trim.lead.take() else {
            return;
        };
        if cut.get() == 0 || cut.get() > self.content.text.len() {
            return;
        }
        self.content.text.drain(..cut.get());
        for item in self.content.items.as_mut_slice() {
            item.start = TextOffset::new(item.start.get().saturating_sub(cut.get()));
            item.end = TextOffset::new(item.end.get().saturating_sub(cut.get()));
        }
        self.content.record(|map| map.drain_front(cut));
        if self.first_line.mirror == Mirror::Writing {
            self.content.first_line_source_mut().drain_front(cut);
        }
    }

    /// Applies the block's trailing trim, removing kept white space from
    /// the first segment break after the last content.
    pub(super) fn trim_end(&mut self) {
        let text = &self.content.text;
        let content_end = text.trim_end_matches(is_document_white_space).len();
        // Atomic inlines and forced breaks are content; never cut them.
        let last_object = self
            .content
            .items
            .as_slice()
            .iter()
            .rev()
            .find(|item| matches!(item.kind, ItemKind::Atomic | ItemKind::Break))
            .map_or(0, |item| item.end.get());
        let from = content_end.max(last_object);
        let Some(cut) = text.get(from..).and_then(|tail| tail.find('\n')) else {
            return;
        };
        let cut = from + cut;
        self.content.text.truncate(cut);
        let cut = TextOffset::new(cut);
        for item in self.content.items.as_mut_slice() {
            item.start = item.start.min(cut);
            item.end = item.end.min(cut);
        }
        self.content.record(|map| map.cut_end(cut));
        if self.content.first_line_source().is_some() {
            self.content.first_line_source_mut().truncate(cut);
        }
    }

    /// Appends `text`, extending `item`, the last text-holding item, and
    /// mirrors it. The caller has checked the room.
    pub(super) fn append(&mut self, item: ItemId, text: &str) {
        let at = TextOffset::new(self.content.text.len());
        self.content.text.push_str(text);
        self.extend(item);
        self.mirror(text.len(), |writer| {
            writer.content.first_line_source_mut().push_same(at, text);
        });
    }

    /// Extends `item`, the last text-holding item, to the text's end.
    fn extend(&mut self, item: ItemId) {
        let end = TextOffset::new(self.content.text.len());
        if let Some(item) = self.content.items.get_mut(item) {
            item.end = end;
        }
    }

    /// Returns the current text node's item, pushing one if needed, or
    /// `None` if there is no text node or no room.
    fn open_item(&mut self) -> Option<ItemId> {
        if let Some(item) = self.text_node.item {
            return Some(item);
        }
        let node = self.text_node.node?;
        let item = self.push_item(ItemKind::Text, node, ItemFlags::NONE)?;
        self.text_node.item = Some(item);
        Some(item)
    }

    /// Writes a generated U+200B as its own `GENERATED` text item of `node`.
    ///
    /// It serves `<wbr>`, a wrap opportunity kept by collapsing, and an
    /// empty combined unit. Like Blink's control item, its own item lets
    /// analysis tell it from a source U+200B: a source one shapes with its
    /// text, a generated one ends a shaping run. Text after it takes a new
    /// item. The caller has checked the text room; if no item fits, nothing
    /// is written.
    ///
    /// Its offset map unit is empty in the source, at `source`.
    pub(super) fn generated_break(&mut self, node: NodeId, source: u32) {
        if let Some(item) = self.push_item(ItemKind::Text, node, ItemFlags::GENERATED) {
            let at = TextOffset::new(self.content.text.len());
            self.append(item, "\u{200B}");
            self.content.record(|map| map.generated(source..source, at));
        }
    }
}

/// Records offset map units for source `src` at `source`, transformed into
/// `written` at `mark`, one base character with its marks at a time.
///
/// It serves pieces whose context changed the mapping, as when a Greek
/// capital drops an accent: `Ϊ́Ρ` uppercases to `ΪΡ`, and the first letter
/// keeps its own unit, as Chrome maps a selection of it. Returns false if
/// the clusters mapped alone don't make `written`, or the transform
/// capitalizes; the caller then undoes the units and maps the piece whole.
#[cold]
#[inline(never)]
fn transformed_clusters(
    map: &mut Map,
    written: &str,
    src: &str,
    transformer: TextTransformer,
    source: u32,
    mark: TextOffset,
) -> bool {
    if transformer.capitalizes() {
        return false;
    }
    let (mut made, mut scratch) = (String::new(), String::new());
    let mut rest = src;
    let mut at = 0;
    while let Some(first) = rest.chars().next() {
        work::step();
        let len = first.len_utf8()
            + rest[first.len_utf8()..]
                .chars()
                .take_while(|&ch| is_mark(ch))
                .map(char::len_utf8)
                .sum::<usize>();
        let (cluster, after) = rest.split_at(len);
        made.clear();
        transformer.write(cluster, ' ', &mut scratch, &mut made);
        let Some(tail) = written.get(at..) else {
            return false;
        };
        if !tail.starts_with(made.as_str()) {
            return false;
        }
        let from = source.saturating_add(index_to_u32(src.len() - rest.len()));
        let to = from.saturating_add(index_to_u32(len));
        if made == cluster {
            map.identity(from, len, TextOffset::new(mark.get() + at), false);
        } else {
            map.variable(from..to, TextOffset::new(mark.get() + at));
        }
        at += made.len();
        rest = after;
    }
    at == written.len()
}

/// Whether `ch` is document white space, which `white-space-trim` trims:
/// space, tab, segment break, or CR (a space in all respects in CSS Text
/// 3). A form feed is not; it is drawn and forces a break.
fn is_document_white_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

/// Returns the length of the prefix of `text` that can be written as is.
///
/// The prefix ends at the first special character: LF, CR, VT, FF, NEL,
/// U+2028, U+2029, and if `spaces` (collapsing or discarded), space and
/// tab. With `lone_spaces`, a space between two plain characters is plain.
/// The result is a character boundary, since no other character's
/// continuation bytes match these.
fn plain_len(text: &str, spaces: bool, lone_spaces: bool) -> usize {
    const ONES: u64 = u64::from_le_bytes([1; 8]);
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(rest) = bytes.get(at..).filter(|rest| !rest.is_empty()) {
        work::step();
        let x = match rest.first_chunk::<8>() {
            Some(&eight) => u64::from_le_bytes(eight),
            // Pad the tail with `a` in a register; padding through memory
            // would stall the load on the stores.
            None => rest
                .iter()
                .rev()
                .fold(ONES * u64::from(b'a'), |x, &byte| x << 8 | u64::from(byte)),
        };
        // Every special character starts with a byte <= 0x20, or with C2 or
        // E2, which differ in one bit. Such bytes are flagged; bytes above
        // the first flagged one may be flagged falsely, so each is checked.
        let low = x.wrapping_sub(ONES * 0x21) & !x;
        let lead = (x & (ONES * 0xDF)) ^ (ONES * 0xC2);
        let lead = lead.wrapping_sub(ONES) & !lead;
        let mut flagged = (low | lead) & (ONES * 0x80);
        while flagged != 0 {
            let candidate = at + (flagged.trailing_zeros() / 8) as usize;
            flagged &= flagged - 1;
            let Some(&byte) = bytes.get(candidate) else {
                break;
            };
            let lone = byte == b' '
                && lone_spaces
                && candidate > 0
                && bytes
                    .get(candidate + 1)
                    .is_some_and(|&next| !is_special(bytes, candidate + 1, next, true));
            if !lone && is_special(bytes, candidate, byte, spaces) {
                return candidate;
            }
        }
        at += 8;
    }
    bytes.len()
}

/// Whether `byte`, at `at` in `bytes`, starts a special character (see
/// [`plain_len`]).
#[inline(always)]
fn is_special(bytes: &[u8], at: usize, byte: u8, spaces: bool) -> bool {
    if byte > b' ' && byte & 0xDF != 0xC2 {
        return false;
    }
    match byte {
        b'\n' | b'\r' | b'\x0B' | b'\x0C' => true,
        b' ' | b'\t' => spaces,
        // NEL, U+0085, is C2 85.
        0xC2 => bytes.get(at + 1) == Some(&0x85),
        // U+2028 and U+2029 are E2 80 A8 and E2 80 A9.
        0xE2 => bytes.get(at + 1) == Some(&0x80) && matches!(bytes.get(at + 2), Some(0xA8 | 0xA9)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::{is_special, plain_len};

    /// `plain_len`, one byte at a time.
    fn scan(text: &str, spaces: bool, lone_spaces: bool) -> usize {
        let bytes = text.as_bytes();
        let stops = |at: usize, byte: u8| {
            let lone = byte == b' '
                && lone_spaces
                && at > 0
                && bytes
                    .get(at + 1)
                    .is_some_and(|&next| !is_special(bytes, at + 1, next, true));
            !lone && is_special(bytes, at, byte, spaces)
        };
        (0..bytes.len())
            .find(|&at| stops(at, bytes[at]))
            .unwrap_or(bytes.len())
    }

    #[test]
    fn plain_text_ends_where_a_scan_a_byte_at_a_time_ends_it() {
        // Every special character, characters sharing their lead bytes,
        // and words that cross eight-byte chunks.
        let pieces = [
            "a",
            " ",
            "\t",
            "\n",
            "\r",
            "\u{B}",
            "\u{C}",
            "\u{1}",
            "\u{85}",
            "\u{A9}",
            "\u{2028}",
            "\u{2029}",
            "\u{2027}",
            "\u{E2}",
            "é",
            "日本",
            "\u{200B}",
            "abcdefghij",
        ];
        let mut state = 1_u64;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..next() % 24 {
                text.push_str(pieces[next() % pieces.len()]);
            }
            for (spaces, lone_spaces) in [(true, true), (true, false), (false, false)] {
                assert_eq!(
                    plain_len(&text, spaces, lone_spaces),
                    scan(&text, spaces, lone_spaces),
                    "{text:?}, spaces {spaces}, lone spaces {lone_spaces}",
                );
            }
        }
    }
}
