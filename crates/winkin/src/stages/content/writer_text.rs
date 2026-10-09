//! Writing text: white space collapsing, transforms, masks, forced breaks, the
//! offset map and the first line's text.

use alloc::boxed::Box;
use core::mem;

use icu_segmenter::GraphemeClusterSegmenter;

use super::collapse::{Anchor, Effect, Event, Run, ZWSP};
use super::transform::{TextTransformer, Transforms};
use super::{
    Content, ContentExtras, ContentFlags, ContentWriter, FirstLetter, Item, ItemFlags, ItemId,
    ItemKind, Mirror, NodeId,
};
use crate::data::{TextOffset, index_to_u32};
use crate::style::WhiteSpaceCollapse;
use crate::work;

impl ContentWriter<'_> {
    /// Writes `text` for the current text node, under its white space rules,
    /// transformed as `transforms` say.
    ///
    /// The text is transformed a piece at a time, between the white space
    /// that collapsing reads. No transform makes or takes collapsible white
    /// space, and no case mapping reads its context past it. So the result
    /// equals transforming the whole text and then collapsing it, as Chrome
    /// does. Each piece knows the character before it, which capitalize
    /// reads.
    pub(super) fn write_text(
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
            let before = mem::replace(&mut self.last_char, mask);
            self.write_masked(mask, transforms, before, text);
            return;
        }
        // What the next text reads before it, whatever of this is kept.
        let mut before = mem::replace(&mut self.last_char, last);
        let mut rest = text;
        while !rest.is_empty() {
            if self.full {
                self.report.drop_bytes(rest.len());
                return;
            }
            let plain = plain_len(rest, !mode.keeps_spaces());
            // `plain_len` stops only at the start of a character.
            let (content, tail) = rest.split_at(plain);
            if !content.is_empty() {
                let kept = self.write_content(content, None, transforms, before);
                if kept < content.len() {
                    self.report.drop_bytes(content.len() - kept + tail.len());
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
                // The CR of a CRLF is part of the segment break its LF
                // makes, whatever the mode makes of that, as HTML normalizes
                // a CRLF to an LF before CSS sees it.
                '\r' if rest.starts_with('\n') => {
                    self.collapsed(len);
                    true
                }
                // Inside a ruby container no segment break is kept: a column
                // is one unit of a line, and no paragraph ends inside one.
                // Where only breaks were kept it collapses as white space,
                // and where spaces are kept it is one, as Blink has it
                // (`should_not_preserve_newline` in ruby).
                '\n' => match (mode, self.ruby.is_some()) {
                    (Ws::Discard, _) => {
                        self.collapsed(len);
                        true
                    }
                    (Ws::Collapse, _) | (Ws::PreserveBreaks, true) => {
                        self.white_space(true, wraps, len);
                        true
                    }
                    // A segment break written as a space: where spaces
                    // alone are kept, and in ruby where spaces are kept.
                    (Ws::PreserveSpaces, _) | (Ws::Preserve | Ws::BreakSpaces, true) => {
                        self.write_content(" ", Some(len), Transforms::NONE, ch) > 0
                    }
                    // A kept segment break: `preserve`, `break-spaces` and
                    // `pre-line`.
                    (Ws::Preserve | Ws::BreakSpaces | Ws::PreserveBreaks, false) => {
                        self.write_forced_break("\n", true)
                    }
                },
                // A space or a tab, which reaches here only where it
                // collapses or is discarded, or a lone CR. CSS Text 3 makes
                // a lone CR a space in all respects: collapsible where spaces
                // collapse, and kept, hanging and breaking as one where they
                // are kept.
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
                        self.write_content(" ", Some(len), transforms, previous) > 0
                    }
                },
                // VT, FF, NEL, U+2028 or U+2029 forces a break in every
                // mode (CSS Text 3, section 5.1). In a ruby container, where
                // no paragraph ends, it is a space, as a `<br>` there is.
                _ if self.ruby.is_some() => {
                    let kept = self.write_content(" ", Some(len), Transforms::NONE, ch) > 0;
                    if kept {
                        self.collapser.step(Event::ForcedBreak);
                    }
                    kept
                }
                _ => self.write_forced_break(ch.encode_utf8(&mut [0; 4]), false),
            };
            if !written {
                self.report.drop_bytes(ch.len_utf8() + rest.len());
                return;
            }
        }
    }

    /// Writes `text` for the current text node masked by `-webkit-text-security`:
    /// one `mask` for each grapheme of the text as `transforms` transform it.
    ///
    /// Chrome masks the text before white space is processed
    /// (`LayoutText::SecureText`), so every space, tab and segment break is
    /// masked like any other character: none collapses, and none breaks a
    /// line. Each of the caller's graphemes is one offset map unit, so a mask
    /// is one caret stop. `before` is the character before `text`, which
    /// capitalize reads.
    fn write_masked(&mut self, mask: char, transforms: Transforms, mut before: char, text: &str) {
        self.content_arrives(mask, false);
        let mut buffer = [0; 4];
        let mask: &str = mask.encode_utf8(&mut buffer);
        self.reserve_text(text.len().saturating_mul(mask.len()));
        let Some(item) = self.open_item() else {
            self.report.drop_bytes(text.len());
            return;
        };
        let mut boundaries = GraphemeClusterSegmenter::new().segment_str(text);
        // The segmenter says 0 first.
        let mut start = boundaries.next().unwrap_or(0);
        for end in boundaries {
            work::step();
            let grapheme = text.get(start..end).unwrap_or_default();
            let count = match transforms.own {
                Some(transformer) => self.transformed_graphemes(transformer, grapheme, before),
                None => 1,
            };
            if !self.has_text_room(count.saturating_mul(mask.len())) {
                self.full = true;
                self.report.drop_bytes(text.len() - start);
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
                let end = self.source.saturating_add(index_to_u32(grapheme.len()));
                self.content
                    .record(|map| map.variable(self.source..end, at));
                self.source = end;
            }
            before = grapheme.chars().next_back().unwrap_or(before);
            start = end;
        }
    }

    /// Returns how many graphemes `transformer` makes of `grapheme`, with
    /// `before` the character before it.
    ///
    /// It writes the transform past the text's end and takes it back, so it
    /// needs no buffer of its own. Where that does not fit, it counts one.
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
        transformer.write(grapheme, before, self.words, &mut self.content.text);
        let written = self.content.text.get(mark..).unwrap_or_default();
        // The segmenter says 0 first, and nothing more for no text.
        let count = GraphemeClusterSegmenter::new()
            .segment_str(written)
            .count()
            .saturating_sub(1);
        self.content.text.truncate(mark);
        count
    }

    /// Steps the collapser over a character of collapsible white space in the
    /// current text node, `len` bytes of the caller's.
    fn white_space(&mut self, segment_break: bool, wraps: bool, len: usize) {
        let anchor = self.anchor();
        let at = TextOffset::new(self.content.text.len());
        let source = self.source..self.source.saturating_add(index_to_u32(len));
        self.source = source.end;
        match self.collapser.step(Event::Space {
            segment_break,
            wraps,
            anchor,
        }) {
            Effect::Began => {
                // The run's item, pushed now if the node has none, so that its
                // id is the one the anchor named.
                if self.open_item.is_none() {
                    let item = self.open_item();
                    debug_assert_eq!(item, anchor.map(|anchor| anchor.item));
                }
                self.run_unit = self.content.record(|map| map.run(source, at)).flatten();
            }
            Effect::GenerateBreak => {
                if !self.has_text_room(ZWSP.len_utf8()) {
                    self.full = true;
                } else if let Some(node) = self.text_node {
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

    /// Records `len` bytes of the text node's text left out: collapsed away
    /// or discarded, with nothing written for them.
    fn collapsed(&mut self, len: usize) {
        let at = TextOffset::new(self.content.text.len());
        let source = self.source..self.source.saturating_add(index_to_u32(len));
        self.source = source.end;
        self.content.record(|map| map.collapsed(source, at));
    }

    /// Records in the offset map `len` bytes of the text node's text, written
    /// as given from `at` (see [`is_joined`](Self::is_joined)).
    fn written_as_given(&mut self, len: usize, at: TextOffset) {
        let joins = self.is_joined(at);
        self.content
            .record(|map| map.identity(self.source, len, at, joins));
        self.source = self.source.saturating_add(index_to_u32(len));
    }

    /// Returns whether text written as given from `at` joins the offset map's
    /// unit before it.
    ///
    /// It does unless the block trims its kept white space and the text
    /// follows a kept segment break, where the trim may cut.
    fn is_joined(&self, at: TextOffset) -> bool {
        !(self.block_trims
            && self
                .content
                .text
                .get(..at.get())
                .is_some_and(|before| before.ends_with('\n')))
    }

    /// Returns where a run of white space beginning now would put its space,
    /// or `None` if its item would not fit.
    fn anchor(&self) -> Option<Anchor> {
        let item = match self.open_item {
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

    /// Writes a forced break in the text node's item, `written` as the caller
    /// wrote it. Returns false if it did not fit.
    ///
    /// The break is a kept segment break, `\n` (`kept_segment_break`), or a
    /// character that forces one in every white space mode: VT, FF, NEL,
    /// U+2028 or U+2029. It takes the collapsible white space on both sides,
    /// as every forced break does, and ends the first paragraph.
    ///
    /// The block's leading trim treats a kept segment break as white space
    /// and may cut after it, so it starts an offset map unit where the block
    /// trims. The trim treats the other characters as content.
    fn write_forced_break(&mut self, written: &str, kept_segment_break: bool) -> bool {
        if !self.has_text_room(written.len()) {
            self.full = true;
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
            !self.block_trims
        } else {
            self.is_joined(at)
        };
        self.content
            .record(|map| map.identity(self.source, written.len(), at, joins));
        self.source = self.source.saturating_add(index_to_u32(written.len()));
        if self.lead_trim.is_some() {
            // Kept white space the block's leading trim is still to take,
            // this break with it: the first paragraph has not ended.
            self.lead_trim = Some(TextOffset::new(self.content.text.len()));
        } else {
            self.mirror = Mirror::Done;
        }
        true
    }

    /// Writes `content`, which holds no collapsible white space, for the
    /// current text node, transformed as `transforms` say.
    ///
    /// `before` is the caller's character before it. Returns how many bytes
    /// were kept: all of them unless the text or the items ran out. The first
    /// line's text takes it too, under its own transform (see `first_line`).
    ///
    /// `stands` is how many bytes of the caller's text it stands for, where
    /// it is not the caller's text itself: a space written for a carriage
    /// return or a segment break.
    fn write_content(
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
        if transforms.first_line_differs() && self.mirror == Mirror::Waiting {
            // The first text the first line transforms its own way: the
            // first line's text starts as the text so far.
            let Content { text, extras, .. } = &mut *self.content;
            let extras = extras.get_or_insert_with(Box::default);
            extras.first_line_source.start(text);
            self.content.flags.insert(ContentFlags::TRANSFORM_SIDE_TEXT);
            self.mirror = Mirror::Writing;
        }
        let mark = TextOffset::new(self.content.text.len());
        let room = self.limits.text.saturating_sub(mark.get());
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
            // Room for the most this transform can make of it: written whole,
            // in its context, and each of the caller's characters mapped to
            // what it became.
            Some(transformer) if content.len().saturating_mul(transformer.max_growth()) <= room => {
                transformer.write(content, before, self.words, &mut self.content.text);
                self.extend(item);
                if stands.is_none() {
                    self.transformed(mark, content, transformer, before);
                }
                content.len()
            }
            // Near the limit, a character at a time, each on its own, until
            // one does not fit: the text is cut after the last of the
            // caller's characters whose transform fits.
            Some(transformer) => {
                let mut kept = 0;
                let mut before = before;
                let mut one = [0u8; 4];
                for ch in content.chars() {
                    let back_to = self.content.text.len();
                    let alone = ch.encode_utf8(&mut one);
                    transformer.write(alone, before, self.words, &mut self.content.text);
                    if self.content.text.len() > self.limits.text {
                        self.content.text.truncate(back_to);
                        break;
                    }
                    kept += ch.len_utf8();
                    before = ch;
                    if stands.is_none() {
                        self.character_written(ch, TextOffset::new(back_to));
                    }
                }
                self.extend(item);
                kept
            }
        };
        // What stands for the caller's text: one of the caller's characters
        // written as another, or others.
        if let Some(len) = stands
            && kept > 0
        {
            self.character_stands(len, mark);
        }
        if self.mirror == Mirror::Writing {
            // No longer than the content's text may be; past that, which
            // only a first paragraph near the limit reaches, the first line
            // draws what it has and nothing further.
            let most = content.len().saturating_mul(transforms.max_growth());
            if self.mirror_room(most) {
                self.mirror_content(content, kept, mark, transforms, before);
            } else {
                self.mirror = Mirror::Done;
            }
        }
        if kept < content.len() {
            self.full = true;
        }
        kept
    }

    /// Records the offset map's units for `src`, the caller's text, which a
    /// transform wrote from `mark` to the text's end.
    ///
    /// `before` is the caller's character before it. A character the
    /// transform made one of the same length is written as given. One it made
    /// another length takes a `Variable` unit. Where the context mapped the
    /// characters otherwise than each alone (a Greek accent dropped under a
    /// capital), the whole piece is one unit.
    fn transformed(
        &mut self,
        mark: TextOffset,
        src: &str,
        transformer: TextTransformer,
        before: char,
    ) {
        let source = self.source;
        let end = source.saturating_add(index_to_u32(src.len()));
        let joins = self.is_joined(mark);
        self.source = end;
        let Content { text, extras, .. } = &mut *self.content;
        let Some(map) = extras.as_deref_mut().and_then(ContentExtras::recorded_map) else {
            return;
        };
        let written = text.get(mark.get()..).unwrap_or_default();
        // Case mapping makes one ASCII character of one: the text as given.
        if src.is_ascii() && written.is_ascii() && src.len() == written.len() {
            map.identity(source, src.len(), mark, joins);
            return;
        }
        // Whether what each character made alone adds up to what the
        // piece made in its context, under the text's own transform alone.
        let own = Transforms::new(Some(transformer), None);
        let mut made = written.chars();
        let mut fits = true;
        own.counts(src, before, self.words, &mut |_, count, _| {
            for _ in 0..count {
                fits &= made.next().is_some();
            }
        });
        if !fits || made.next().is_some() {
            map.variable(source..end, mark);
            return;
        }
        let mut made = written.chars();
        let (mut from, mut at) = (source, mark);
        own.counts(src, before, self.words, &mut |ch, count, _| {
            let bytes: usize = (0..count)
                .map(|_| made.next().map_or(0, char::len_utf8))
                .sum();
            let len = index_to_u32(ch.len_utf8());
            if count == 1 && bytes == ch.len_utf8() {
                map.identity(from, bytes, at, joins || at != mark);
            } else {
                map.variable(from..from.saturating_add(len), at);
            }
            from = from.saturating_add(len);
            at = TextOffset::new(at.get() + bytes);
        });
    }

    /// Records the offset map's unit for `ch`, one of the caller's
    /// characters, which a transform wrote alone from `at` to the text's end.
    ///
    /// The unit is written as given if the transform made one character of
    /// the same length, and `Variable` otherwise.
    fn character_written(&mut self, ch: char, at: TextOffset) {
        let written = self.content.text.get(at.get()..).unwrap_or_default();
        let one = written.chars().count() == 1;
        if one && written.len() == ch.len_utf8() {
            self.written_as_given(ch.len_utf8(), at);
        } else {
            self.character_stands(ch.len_utf8(), at);
        }
    }

    /// Records the offset map's unit for what was written from `at` to the
    /// text's end, standing for `len` bytes of the caller's text.
    ///
    /// The unit is written as given if it is as long, and `Variable`
    /// otherwise.
    fn character_stands(&mut self, len: usize, at: TextOffset) {
        let written = self.content.text.len() - at.get();
        if written == len {
            self.written_as_given(len, at);
        } else {
            let end = self.source.saturating_add(index_to_u32(len));
            self.content
                .record(|map| map.variable(self.source..end, at));
            self.source = end;
        }
    }

    /// Returns whether the first line's text has room for `bytes` more. It
    /// holds no more than the content's text may.
    fn mirror_room(&self, bytes: usize) -> bool {
        let held = self
            .content
            .first_line_source()
            .map_or(0, |source| source.text.len());
        held.saturating_add(bytes) <= self.limits.text
    }

    /// Copies into the first line's text what
    /// [`write_content`](Self::write_content) wrote from content offset
    /// `mark`, of which `kept` bytes of the caller's `content` were kept.
    ///
    /// Where the first line transforms the text its own way, it takes that
    /// transform of the caller's text instead, aligned one caller's character
    /// at a time. A piece cut at the text's limit is drawn on the first line
    /// as the text has it.
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
            first_line_source
                .push_transformed(mark, written, content, before, transforms, self.words);
        } else {
            first_line_source.push_same(mark, written);
        }
    }

    /// Settles white space as content arrives.
    ///
    /// A pending run writes its space. The block's leading trim cuts, unless
    /// the content is kept white space (`white`).
    pub(super) fn content_arrives(&mut self, first: char, white: bool) {
        if let Effect::WriteSpace(run) = self.collapser.step(Event::Content { first }) {
            self.write_space(run);
        }
        if !white {
            self.lead_trim_ends();
            // A first letter asked for from here on would not be first.
            if self.first_letter == FirstLetter::Unarmed {
                self.first_letter = FirstLetter::Done;
            }
        }
    }

    /// Tells the collapser whether the text that follows is `combined`.
    ///
    /// A combined unit starting settles the run before it, as an atomic
    /// inline does.
    pub(super) fn combined_text_arrives(&mut self, combined: bool) {
        if let Effect::WriteSpace(run) = self.collapser.step(Event::Combined(combined)) {
            self.write_space(run);
        }
    }

    /// Writes the space `run` owes, where it began.
    fn write_space(&mut self, run: Run) {
        let unit = self.run_unit.take();
        if !self.has_text_room(1) {
            self.report.drop_bytes(1);
            self.full = true;
            return;
        }
        let at = run.at.get();
        let Content { text, items, .. } = &mut *self.content;
        // The run's item, which takes the space, and every item written
        // since, which moves along by it.
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
        if self.mirror == Mirror::Writing {
            if self.mirror_room(1) {
                self.content.first_line_source_mut().insert_space(run.at);
            } else {
                self.mirror = Mirror::Done;
            }
        }
    }

    /// Ends the block's leading trim here, removing what was written through
    /// its last segment break.
    pub(super) fn lead_trim_ends(&mut self) {
        let Some(cut) = self.lead_trim.take() else {
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
        if self.mirror == Mirror::Writing {
            self.content.first_line_source_mut().drain_front(cut);
        }
    }

    /// Applies the block's trailing trim, removing kept white space from the
    /// first segment break after its last content.
    pub(super) fn trim_end(&mut self) {
        let text = &self.content.text;
        let content_end = text.trim_end_matches(is_document_white_space).len();
        // Never into an atomic inline or a forced break, which are content.
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

    /// Appends `text` to the text, extending `item`, which is the last item
    /// that holds text. The room was checked.
    pub(super) fn append(&mut self, item: ItemId, text: &str) {
        let at = TextOffset::new(self.content.text.len());
        self.content.text.push_str(text);
        self.extend(item);
        if self.mirror == Mirror::Writing {
            if self.mirror_room(text.len()) {
                self.content.first_line_source_mut().push_same(at, text);
            } else {
                self.mirror = Mirror::Done;
            }
        }
    }

    /// Extends `item`, the last item that holds text, to the text's end.
    fn extend(&mut self, item: ItemId) {
        let end = TextOffset::new(self.content.text.len());
        if let Some(item) = self.content.items.get_mut(item) {
            item.end = end;
        }
    }

    /// Returns the current text node's item, pushed now if it has none, or
    /// `None` if there is no text node or no room.
    fn open_item(&mut self) -> Option<ItemId> {
        if let Some(item) = self.open_item {
            return Some(item);
        }
        let node = self.text_node?;
        let item = self.push_item(ItemKind::Text, node, ItemFlags::NONE)?;
        self.open_item = Some(item);
        Some(item)
    }

    /// Writes a break opportunity the builder generated, U+200B, as a text
    /// item of `node`'s own, flagged generated.
    ///
    /// It is a `<wbr>`'s, or the one collapsing keeps. It takes an item of
    /// its own, as Blink's control item does, so that analysis tells it from
    /// a U+200B the caller wrote: that one is shaped with its text, and this
    /// one ends a shaping run. The text after it in `node` takes a new item.
    /// The caller checked the room for its text. Where the items are full,
    /// nothing is written, as for any item that does not fit.
    ///
    /// It holds none of the caller's text: its offset map unit is empty
    /// there, at `source`, where the text before it has got to.
    pub(super) fn generated_break(&mut self, node: NodeId, source: u32) {
        if let Some(item) = self.push_item(ItemKind::Text, node, ItemFlags::GENERATED) {
            let at = TextOffset::new(self.content.text.len());
            self.append(item, "\u{200B}");
            self.content.record(|map| map.generated(source..source, at));
        }
    }
}

/// Returns whether `ch` is CSS's document white space, which
/// `white-space-trim` trims.
///
/// That is a space, tab or segment break, and a carriage return, which CSS
/// Text 3 makes a space in all respects. A form feed is not: it is drawn,
/// and forces a break.
fn is_document_white_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

/// Returns how many bytes of `text` come before the first character the
/// writer handles on its own.
///
/// Those characters are a segment break, a carriage return, a character
/// that forces a line break (VT, FF, NEL, U+2028, U+2029), and, if `spaces`
/// (where they collapse or are discarded), a space or a tab. Everything
/// before is written as it is. The result is always a character boundary:
/// each of these is one character, and no other character's later bytes
/// match.
fn plain_len(text: &str, spaces: bool) -> usize {
    let bytes = text.as_bytes();
    for (at, &byte) in bytes.iter().enumerate() {
        work::step();
        let special = match byte {
            b'\n' | b'\r' | b'\x0B' | b'\x0C' => true,
            b' ' | b'\t' => spaces,
            // NEL, U+0085, is C2 85.
            0xC2 => bytes.get(at + 1) == Some(&0x85),
            // U+2028 and U+2029 are E2 80 A8 and E2 80 A9.
            0xE2 => {
                bytes.get(at + 1) == Some(&0x80) && matches!(bytes.get(at + 2), Some(0xA8 | 0xA9))
            }
            _ => false,
        };
        if special {
            return at;
        }
    }
    bytes.len()
}
