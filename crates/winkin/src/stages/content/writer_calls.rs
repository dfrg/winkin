//! The calls the builder makes on the writer: text, containers, atomic
//! inlines, floats, absolutely positioned boxes, breaks and `finish`.

#[cfg(debug_assertions)]
use super::check;
use super::collapse::{Event, ZWSP};
use super::map::Map;
use super::transform::Transforms;
use super::writer::OpenId;
use super::{
    Absolute, Atomic, AtomicId, BreakClearance, ContainerKind, Content, ContentFlags,
    ContentWriter, FirstLetter, Float, ItemFlags, ItemKind, LoweredNode, MAX_RUBY_DEPTH, Mirror,
    NodeFacts, NodeId, NodeKey, NodeKind, OBJECT, Open, TextFlags,
};
use crate::build::{BoxSize, BuildReport, Clear, FloatSide, OriginalDisplay};
use crate::data::{HashIndex, TextOffset, hash_one};
use crate::style::{
    ComputedStyle, FirstLineVariant, RubyGroup, RubyPosition, TextCase, TextCombineUpright,
    WhiteSpaceTrim,
};
use alloc::boxed::Box;

impl ContentWriter<'_> {
    /// Writes text from the text node `key`. Where a first letter is asked
    /// for and not yet found, the text splits at the letter's end.
    pub(crate) fn text(&mut self, key: NodeKey, text: &str) {
        if !self.content.flags.contains(ContentFlags::MATH_AUTO) {
            self.write_text_call(key, text, false);
            return;
        }
        self.math_auto_text(key, text);
    }

    /// Resolves a source node's length before writing its mathematical transform.
    #[inline(never)]
    fn math_auto_text(&mut self, key: NodeKey, text: &str) {
        if self.math_key == Some(key) {
            self.write_text_call(key, text, false);
            self.math_key = Some(key);
            return;
        }
        if let Some((held_key, ch)) = self.math_text.take() {
            if held_key == key && text.is_empty() {
                self.math_text = Some((held_key, ch));
                return;
            }
            self.write_text_call(held_key, ch.encode_utf8(&mut [0; 4]), held_key != key);
            if held_key == key {
                self.write_text_call(key, text, false);
                self.math_key = Some(key);
                return;
            }
        }
        let letter_math = match self.first_letter {
            FirstLetter::Armed {
                style, first_line, ..
            } => {
                matches!(style.text.transform.case, TextCase::MathAuto)
                    || first_line
                        .is_some_and(|s| matches!(s.text.transform.case, TextCase::MathAuto))
            }
            _ => false,
        };
        let math = self.container_transforms().has_math_auto() || letter_math;
        if math {
            let mut chars = text.chars();
            if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
                if self.text_node_keyed(key).is_none() {
                    self.end_text();
                }
                self.math_text = Some((key, ch));
                return;
            }
        }
        self.write_text_call(key, text, false);
        self.math_key = (math && !text.is_empty()).then_some(key);
    }

    /// Writes one call, `single` where `text` is the whole of a one-character
    /// source node `math-auto` may map.
    #[inline]
    pub(super) fn write_text_call(&mut self, key: NodeKey, text: &str, single: bool) {
        match self.first_letter {
            FirstLetter::Armed { .. } | FirstLetter::Punctuation { .. } if !self.full => {
                self.text_with_first_letter(key, text, single);
            }
            _ => self.plain_text(key, text, single),
        }
    }

    /// Writes text from the text node `key`, with no first letter in it.
    pub(super) fn plain_text(&mut self, key: NodeKey, text: &str, single: bool) {
        let continues = self.text_node_keyed(key).is_some();
        if self.full {
            // Nothing more fits: the text goes, and its node if it needed one.
            self.report.drop_bytes(text.len());
            if !continues {
                self.end_text();
                self.report.drop_node();
            }
            return;
        }
        let node = match self.text_node {
            Some(node) if continues => node,
            _ => {
                self.end_text();
                // The rest of a text node the first letter was cut from goes
                // on counting where the letter ended.
                let base = match self.continues.take() {
                    Some((continued, at)) if continued == key => at,
                    _ => 0,
                };
                let around = self.container_facts();
                // A node with room for its item, or neither.
                let node = if self.has_item_room(1) {
                    self.push_node(NodeKind::Text, around, key)
                } else {
                    None
                };
                let Some(node) = node else {
                    self.report.drop_node();
                    self.report.drop_bytes(text.len());
                    return;
                };
                self.text_node = Some(node);
                self.source = base;
                node
            }
        };
        self.write_node_text(node, text, single);
    }

    /// Writes `text` for `node`, the current text node.
    ///
    /// The text follows the white space rules of the node's text facts. It
    /// is transformed as its container says. Where the first-line style
    /// transforms otherwise, the first line's text follows that style.
    pub(super) fn write_node_text(&mut self, node: NodeId, text: &str, single: bool) {
        self.reserve_text(text.len());
        let facts = self.text_facts(node);
        let (mode, wraps) = (facts.collapse, facts.has(TextFlags::WRAPS));
        let combined = facts.combine == TextCombineUpright::All;
        self.combined_text_arrives(combined);
        let transforms = self.container_transforms();
        // The first line's own transform matters only while the first
        // paragraph is being written, where it transforms otherwise.
        let transforms = match self.mirror {
            Mirror::Done => Transforms::new(transforms.own, None),
            Mirror::Waiting | Mirror::Writing => transforms,
        };
        let transforms = if self.content.flags.contains(ContentFlags::MATH_AUTO) {
            transforms.with_node(single, text)
        } else {
            transforms
        };
        self.write_text(mode, wraps, transforms, text);
    }

    /// Opens a container, styled `style` and, where the block has
    /// `::first-line`, `first_line`.
    pub(crate) fn open(
        &mut self,
        container: ContainerKind,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        let container = if container == ContainerKind::Ruby && self.ruby.is_some() {
            if self.annotations == 0 && self.ruby_depth < MAX_RUBY_DEPTH {
                ContainerKind::Ruby
            } else {
                self.flattened_rubies += 1;
                ContainerKind::FlattenedRuby
            }
        } else {
            container
        };
        let trim = style.text.white_space_trim;
        self.open_lowered(container, key, trim, false, |writer, kind| {
            writer.lower_node_styles(style, first_line, kind, container == ContainerKind::Box)
        });
    }

    /// Opens an anonymous ruby container for the annotation `key`, which
    /// opens outside every other, set in `style`.
    ///
    /// Chrome wraps a `ruby-text` box whose parent is no ruby in a ruby
    /// column whose base is empty. The container takes the text facts and
    /// direction of the container it is in, and the ruby properties the
    /// annotation inherits from it, and closes with the annotation.
    fn open_anonymous_ruby(&mut self, key: NodeKey, style: &ComputedStyle<'_>) {
        self.open_lowered(
            ContainerKind::Ruby,
            key,
            WhiteSpaceTrim::NONE,
            true,
            |writer, _| {
                let around = writer.container_facts();
                let direction = writer.direction(writer.container(), FirstLineVariant::Standard);
                let box_ = writer.anonymous_ruby_box(direction, style.ruby);
                LoweredNode {
                    own: NodeFacts { box_, ..around.own },
                    first_line: around.first_line.map(|first| NodeFacts { box_, ..first }),
                    ..around
                }
            },
        );
    }

    /// Opens a container of kind `container` for `key`, trimming white space
    /// as `trim` says, its facts lowered by `lower` for its node's kind.
    ///
    /// An `anonymous` ruby container closes with its annotation.
    fn open_lowered(
        &mut self,
        container: ContainerKind,
        key: NodeKey,
        trim: WhiteSpaceTrim,
        anonymous: bool,
        lower: impl FnOnce(&mut Self, NodeKind) -> LoweredNode,
    ) {
        self.end_text();
        // The first letter is not looked for in ruby, where CSS leaves what
        // an initial letter does undefined (CSS Inline 3, section 7.3.1).
        if container.is_ruby() {
            self.first_letter_ends();
        }
        let dropped = Open {
            node: None,
            kind: container,
            item: self.content.items.next_id(),
            trim: WhiteSpaceTrim::NONE,
            container: self.container(),
            transforms: self.container_transforms(),
            outer: None,
            anonymous,
        };
        // A node and its opening item, and room left for its close.
        if !self.has_node_room() || !self.has_item_room(2) {
            self.report.drop_node();
            self.push_open(dropped);
            return;
        }
        let kind = container.node_kind();
        let lowered = lower(self, kind);
        self.collapser.step(Event::Open { trim });
        let Some(node) = self.push_node(kind, lowered, key) else {
            self.report.drop_node();
            self.push_open(dropped);
            return;
        };
        if container == ContainerKind::Annotation {
            self.annotations += 1;
        } else if container == ContainerKind::Ruby {
            self.ruby_depth += 1;
        }
        let item = self.push_item(container.open_item_kind(), node, ItemFlags::NONE);
        self.reserved += 1;
        self.push_open(Open {
            node: Some(node),
            kind: container,
            item: item.unwrap_or(self.content.items.next_id()),
            trim,
            container: node,
            transforms: lowered.transforms,
            outer: None,
            anonymous,
        });
        match container {
            ContainerKind::Ruby => self.content.flags.insert(ContentFlags::RUBY),
            ContainerKind::Box | ContainerKind::Annotation | ContainerKind::FlattenedRuby => {}
        }
    }

    /// Closes the innermost box, if the innermost container is a box. An
    /// unbalanced close is ignored.
    pub(crate) fn close(&mut self) {
        if self
            .stack
            .last()
            .is_some_and(|open| open.kind == ContainerKind::Box)
        {
            self.pop();
        }
    }

    /// Closes the innermost ruby container, including one in another's base.
    /// A normalized container still consumes its end if its annotation
    /// already closed the box that represented it.
    pub(crate) fn end_ruby(&mut self) {
        if self.flattened_rubies > 0 {
            self.flattened_rubies -= 1;
            self.close_through(ContainerKind::FlattenedRuby, Some(ContainerKind::Ruby));
            return;
        }
        self.close_through(ContainerKind::Ruby, None);
    }

    /// Closes containers down to and including the innermost of `kind`, or
    /// nothing if none is open.
    ///
    /// `stop` is a kind the search does not look past: an annotation is
    /// ended only within the innermost ruby.
    ///
    /// Only a ruby container, an annotation or a nested ruby container is
    /// closed through, and each links to the next one out. The search
    /// follows those links, never the boxes between. However deep the
    /// caller nests boxes in a ruby, a call that closes nothing looks at an
    /// entry or two.
    fn close_through(&mut self, kind: ContainerKind, stop: Option<ContainerKind>) {
        // A ruby container written as a box is still the innermost
        // container to the calls that stop at one.
        let stops = |open: &Open| {
            Some(open.kind) == stop
                || (stop == Some(ContainerKind::Ruby) && open.kind == ContainerKind::FlattenedRuby)
        };
        let found = self
            .ruby_chain()
            .find(|(_, open)| open.kind == kind || stops(open))
            .filter(|(_, open)| open.kind == kind);
        // It closes, and everything open inside it first.
        if let Some((id, _)) = found {
            while id < self.stack.next_id() {
                self.pop();
            }
        }
    }

    /// Opens an annotation of the innermost ruby, closing whatever is open
    /// inside that ruby first (a box in its base, or the annotation before).
    /// Outside any ruby, it opens in an anonymous ruby container of its own.
    ///
    /// The annotation takes `side`, its annotation container's
    /// `ruby-position`, where it has one, and otherwise the innermost ruby
    /// container's, including a nested base container. Only a normalized
    /// container is closed before its annotation opens in the enclosing real
    /// container. Its side is copied while its own style is still available.
    pub(crate) fn annotation(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        side: Option<RubyPosition>,
    ) {
        let found = match self.annotated_ruby() {
            Some(found) => Some(found),
            None => {
                self.open_anonymous_ruby(key, style);
                self.annotated_ruby()
            }
        };
        let Some((ruby, container)) = found else {
            return;
        };
        let nodes = &self.content.nodes;
        let position = side.unwrap_or_else(|| {
            self.content
                .facts
                .box_facts(nodes.box_facts(container.container, FirstLineVariant::Standard))
                .ruby
                .position
        });
        // Everything open inside it closes.
        while self
            .stack
            .ids()
            .next_back()
            .is_some_and(|innermost| innermost > ruby)
        {
            self.pop();
        }
        let style = ComputedStyle {
            ruby: RubyGroup {
                position,
                ..style.ruby
            },
            ..*style
        };
        self.open(ContainerKind::Annotation, key, &style, first_line);
    }

    /// Returns where on the stack the ruby container an annotation opening
    /// now opens in is, and the container whose side it takes.
    ///
    /// The two are found by the links, not by looking at the boxes in
    /// between: the innermost ruby container of either kind, whose side the
    /// annotation takes, and the innermost real ruby container, which is it
    /// or one further out.
    fn annotated_ruby(&self) -> Option<(OpenId, Open)> {
        let mut chain = self.ruby_chain();
        chain
            .find(|(_, open)| {
                matches!(
                    open.kind,
                    ContainerKind::Ruby | ContainerKind::FlattenedRuby
                )
            })
            .and_then(|(at, side)| match side.kind {
                ContainerKind::Ruby => Some((at, side)),
                _ => chain
                    .find(|(_, open)| matches!(open.kind, ContainerKind::Ruby))
                    .map(|(ruby, _)| (ruby, side)),
            })
    }

    /// Closes the innermost annotation, and whatever is open inside it, in
    /// the innermost ruby container. An anonymous container closes with it.
    pub(crate) fn end_annotation(&mut self) {
        self.close_through(ContainerKind::Annotation, Some(ContainerKind::Ruby));
        if self.stack.last().is_some_and(|open| open.anonymous) {
            self.pop();
        }
    }

    /// Writes an atomic inline: one U+FFFC, in a node of its own.
    pub(crate) fn atomic(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        size: BoxSize,
    ) {
        self.end_text();
        // Its character, and the space a pending run may owe before it.
        if !self.has_node_room() || !self.has_item_room(1) || !self.has_text_room(OBJECT.len() + 1)
        {
            self.report.drop_node();
            return;
        }
        let lowered = self.lower_node_styles(style, first_line, NodeKind::Atomic, false);
        // An atomic inline before the first letter leaves none (CSS
        // Pseudo-Elements 4, section 2.2.2).
        self.first_letter_ends();
        self.combined_text_arrives(false);
        self.content_arrives('\u{FFFC}', false);
        let Some(node) = self.push_node(NodeKind::Atomic, lowered, key) else {
            return;
        };
        if let Some(item) = self.push_item(ItemKind::Atomic, node, ItemFlags::NONE) {
            // Its node's offsets 0 and 1, before and after it.
            let at = TextOffset::new(self.content.text.len());
            self.append(item, OBJECT);
            self.content.record(|map| map.generated(0..1, at));
            let writing_mode = self.content.block.writing_mode;
            let atomic = Atomic::new(item, size, style, writing_mode);
            let atomics = &mut self.content.extras_mut().atomics;
            let id = atomics.next_id();
            atomics.push_bounded(atomic, "an atomic has an item, and fits where it does");
            self.note_atomic_key(key, id);
        }
        self.end_leaf(node);
        self.last_char = ' ';
        self.content.flags.insert(ContentFlags::ATOMICS);
    }

    /// Notes that atomic inline `atomic` is keyed `key`, and flags the
    /// content where an atomic inline before it has the same key.
    ///
    /// Once two share a key, it notes no more: the flag is all a search by
    /// key reads.
    fn note_atomic_key(&mut self, key: NodeKey, atomic: AtomicId) {
        if self
            .content
            .flags
            .contains(ContentFlags::SHARED_ATOMIC_KEYS)
        {
            return;
        }
        let hash = hash_one(&key);
        let content = &*self.content;
        let atomics = content.atomics();
        let atomic_keys = self
            .atomic_keys
            .get_or_insert_with(|| Box::new(HashIndex::new()));
        let shared = atomic_keys
            .find::<AtomicId>(hash, |earlier| {
                atomics
                    .get(earlier)
                    .is_some_and(|earlier| content.atomic_key(earlier) == key)
            })
            .is_some();
        if shared {
            self.content.flags.insert(ContentFlags::SHARED_ATOMIC_KEYS);
        } else {
            atomic_keys.insert(hash, atomic);
        }
    }

    /// Writes a float's anchor, which takes no text and is opaque to
    /// collapsing.
    pub(crate) fn float(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        side: FloatSide,
        size: BoxSize,
    ) {
        self.end_text();
        if !self.has_node_room() || !self.has_item_room(1) {
            self.report.drop_node();
            return;
        }
        // `::first-line` styles inline content, which a float is not.
        let lowered = self.lower_node_styles(style, None, NodeKind::Float, false);
        let Some(node) = self.push_node(NodeKind::Float, lowered, key) else {
            return;
        };
        if let Some(item) = self.push_item(ItemKind::Float, node, ItemFlags::NONE) {
            let writing_mode = self.content.block.writing_mode;
            let float = Float::new(item, size, side, style, writing_mode);
            if self.content.extras_mut().floats.push(float).is_none() {
                debug_assert!(false, "a float has an item, and fits where it does");
            }
        }
        self.end_leaf(node);
        self.last_char = ' ';
        self.content.flags.insert(ContentFlags::FLOATS);
    }

    /// Writes an absolutely positioned box's anchor, which takes no text and
    /// is opaque to collapsing, in a node styled by the box it is in.
    ///
    /// Chrome writes it as it writes a float's, as an opaque item with no
    /// text of its own (`AppendOutOfFlowPositioned`). Capitalize reads past
    /// it to the character before, as Chrome does.
    pub(crate) fn absolute(&mut self, key: NodeKey, display: OriginalDisplay) {
        self.end_text();
        if !self.has_node_room() || !self.has_item_room(1) {
            self.report.drop_node();
            return;
        }
        let around = self.container_facts();
        let Some(node) = self.push_node(NodeKind::Absolute, around, key) else {
            return;
        };
        if let Some(item) = self.push_item(ItemKind::Absolute, node, ItemFlags::NONE) {
            let absolute = Absolute { item, display };
            if self.content.extras_mut().absolutes.push(absolute).is_none() {
                debug_assert!(false, "an anchor has an item, and fits where it does");
            }
        }
        self.end_leaf(node);
    }

    /// Writes a forced break: one `\n`, in a node of its own styled by the
    /// box it is in.
    ///
    /// It takes the collapsible white space on both sides. It ends the
    /// first line, and with it any first letter still to come.
    ///
    /// Inside a ruby container it is a space, as Blink makes it
    /// (`kDisableForcedBreakInRubyColumn`). A column is one unit of a line,
    /// and no paragraph ends inside one. The space ends a run of
    /// collapsible white space before it and takes the one after it, as
    /// Blink's space does.
    ///
    /// A break outside ruby that clears floats records `clear`.
    pub(crate) fn line_break(&mut self, key: NodeKey, clear: Option<Clear>) {
        let in_ruby = self.ruby.is_some();
        self.end_text();
        // Its character, and in ruby the space a pending run may owe first.
        let bytes = if in_ruby { 2 } else { 1 };
        if !self.has_node_room() || !self.has_item_room(1) || !self.has_text_room(bytes) {
            self.report.drop_node();
            return;
        }
        self.first_letter_ends();
        if in_ruby {
            self.content_arrives(' ', true);
        } else {
            self.collapser.step(Event::ForcedBreak);
            self.lead_trim_ends();
        }
        let around = self.container_facts();
        let Some(node) = self.push_node(NodeKind::LineBreak, around, key) else {
            return;
        };
        let (kind, text) = if in_ruby {
            (ItemKind::Text, " ")
        } else {
            (ItemKind::Break, "\n")
        };
        if let Some(item) = self.push_item(kind, node, ItemFlags::NONE) {
            let at = TextOffset::new(self.content.text.len());
            self.append(item, text);
            self.content.record(|map| map.generated(0..1, at));
            if !in_ruby {
                self.mirror = Mirror::Done;
                if let Some(clear) = clear {
                    let clearance = BreakClearance { item, clear };
                    self.content
                        .extras_mut()
                        .clearances
                        .push_bounded(clearance, "a clearance has an item, and fits where it does");
                }
            }
        }
        self.end_leaf(node);
        self.last_char = '\n';
        if in_ruby {
            // The white space after it collapses into it, as after a forced
            // break, which leaves the collapser in the same state.
            self.collapser.step(Event::ForcedBreak);
        }
    }

    /// Writes a break opportunity, `<wbr>`, as a generated U+200B.
    ///
    /// It goes in the current text node, or between nodes in the box around
    /// it. It is opaque to collapsing, and ends the source text node
    /// `math-auto` measures, as an element between two text nodes does.
    pub(crate) fn break_opportunity(&mut self) {
        if self.content.flags.contains(ContentFlags::MATH_AUTO) {
            self.end_math_text();
        }
        if self.full {
            return;
        }
        if !self.has_text_room(ZWSP.len_utf8()) {
            self.full = true;
            return;
        }
        self.last_char = ZWSP;
        // How far the text node's text has got; between nodes, how far the
        // last node written got.
        let (node, source) = match self.text_node {
            Some(node) => (node, self.source),
            None => {
                let map = self.content.extras.as_deref().map(|extras| &extras.map);
                (self.container(), map.map_or(0, Map::source_end))
            }
        };
        self.generated_break(node, source);
    }

    /// Closes everything open, settles the white space at the block's end,
    /// and returns what was dropped.
    ///
    /// The content is frozen from here. The builder calls this once, when
    /// it finishes or is dropped, and calls nothing after.
    pub(crate) fn finish(&mut self) -> BuildReport {
        self.end_text();
        self.first_letter_ends();
        self.collapser.step(Event::End);
        while !self.stack.is_empty() {
            self.pop();
        }
        if self.block_trims {
            self.trim_end();
        }
        let end = self.content.items.next_id();
        if let Some(block) = self.content.nodes.get_mut(NodeId::BLOCK) {
            block.end = end;
        }
        #[cfg(debug_assertions)]
        self.debug_check();
        #[cfg(debug_assertions)]
        check(self.content);
        self.report
    }

    /// Returns the content written, for the stages that read it once it is
    /// finished.
    pub(crate) fn content(&self) -> &Content {
        self.content
    }
}
