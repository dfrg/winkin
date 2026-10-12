//! The calls the builder makes on the writer, but text: containers, atomic
//! inlines, floats, absolutely positioned boxes, breaks and `finish`.

use core::mem;

use super::OpenId;
use super::{ContainerKind, ContentWriter, LoweredNode, Mirror, NodeFacts, OBJECT, Open};
use crate::build::{BoxSize, BuildReport, Clear, FloatSide, OriginalDisplay};
use crate::context::Context;
use crate::data::{TextOffset, hash_one};
#[cfg(debug_assertions)]
use crate::stages::content::check;
use crate::stages::content::collapse::{Event, ZWSP};
use crate::stages::content::{
    Absolute, Atomic, AtomicId, BreakClearance, Content, ContentFlags, Float, ItemKind,
    MAX_RUBY_DEPTH, NodeId, NodeKey, NodeKind,
};
use crate::style::{ComputedStyle, FirstLineVariant, RubyGroup, RubyPosition, WhiteSpaceTrim};

impl ContentWriter<'_> {
    /// Opens a container, styled `style` and, where the block has
    /// `::first-line`, `first_line`.
    pub(crate) fn open(
        &mut self,
        container: ContainerKind,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        let container = if container == ContainerKind::Ruby && self.ruby.innermost.is_some() {
            if self.ruby.annotations == 0 && self.ruby.depth < MAX_RUBY_DEPTH {
                ContainerKind::Ruby
            } else {
                self.ruby.flattened += 1;
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
            annotation_close: None,
        };
        // A node and its opening item, and room left for its close.
        if !self.has_node_room() || !self.has_item_room(2) {
            self.room.report.drop_node();
            self.push_open(dropped);
            return;
        }
        let kind = container.node_kind();
        let lowered = lower(self, kind);
        if self
            .push_container(container, kind, key, lowered, trim, anonymous)
            .is_none()
        {
            self.room.report.drop_node();
            self.push_open(dropped);
            return;
        }
        match container {
            ContainerKind::Ruby => self.content.flags.insert(ContentFlags::RUBY),
            ContainerKind::Box | ContainerKind::Annotation | ContainerKind::FlattenedRuby => {}
        }
    }

    /// Closes the innermost box, if the innermost container is a box. An
    /// unbalanced close is ignored.
    pub(crate) fn close(&mut self) {
        if self
            .scratch
            .open
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
        if self.ruby.flattened > 0 {
            self.ruby.flattened -= 1;
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
            while id < self.scratch.open.next_id() {
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
            .scratch
            .open
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
        if self.scratch.open.last().is_some_and(|open| open.anonymous) {
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
            self.room.report.drop_node();
            return;
        }
        let lowered = self.lower_node_styles(style, first_line, NodeKind::Atomic, false);
        // An atomic inline before the first letter leaves none (CSS
        // Pseudo-Elements 4, section 2.2.2).
        self.first_letter_ends();
        self.combined_text_arrives(false);
        self.content_arrives('\u{FFFC}', false);
        let written = self.push_leaf(
            NodeKind::Atomic,
            ItemKind::Atomic,
            lowered,
            key,
            |writer, item| {
                // Its node's offsets 0 and 1, before and after it.
                let at = TextOffset::new(writer.content.text.len());
                writer.append(item, OBJECT);
                writer.content.record(|map| map.generated(0..1, at));
                let writing_mode = writer.content.block.writing_mode;
                let basis = writer.content.basis();
                if style.edges.margin.has_percentage() {
                    writer.content.flags.insert(ContentFlags::PERCENTAGES);
                }
                let atomic = Atomic::new(item, size, style, writing_mode, basis);
                let atomics = &mut writer.content.extras_mut().atomics;
                let id = atomics.next_id();
                atomics.push_bounded(atomic, "an atomic has an item, and fits where it does");
                writer.note_atomic_key(key, id);
            },
        );
        if !written {
            return;
        }
        self.text_node.last_char = ' ';
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
        let atomic_keys = &mut self.scratch.atomic_keys;
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
            self.room.report.drop_node();
            return;
        }
        // `::first-line` styles inline content, which a float is not.
        let lowered = self.lower_node_styles(style, None, NodeKind::Float, false);
        let written = self.push_leaf(
            NodeKind::Float,
            ItemKind::Float,
            lowered,
            key,
            |writer, item| {
                let writing_mode = writer.content.block.writing_mode;
                let basis = writer.content.basis();
                if style.edges.margin.has_percentage() {
                    writer.content.flags.insert(ContentFlags::PERCENTAGES);
                }
                let float = Float::new(item, size, side, style, writing_mode, basis);
                writer
                    .content
                    .extras_mut()
                    .floats
                    .push_bounded(float, "a float has an item, and fits where it does");
            },
        );
        if !written {
            return;
        }
        self.text_node.last_char = ' ';
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
            self.room.report.drop_node();
            return;
        }
        let around = self.container_facts();
        self.push_leaf(
            NodeKind::Absolute,
            ItemKind::Absolute,
            around,
            key,
            |writer, item| {
                writer.content.extras_mut().absolutes.push_bounded(
                    Absolute { item, display },
                    "an anchor has an item, and fits where it does",
                );
            },
        );
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
        let in_ruby = self.ruby.innermost.is_some();
        self.end_text();
        // Its character, and in ruby the space a pending run may owe first.
        let bytes = if in_ruby { 2 } else { 1 };
        if !self.has_node_room() || !self.has_item_room(1) || !self.has_text_room(bytes) {
            self.room.report.drop_node();
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
        let (kind, text) = if in_ruby {
            (ItemKind::Text, " ")
        } else {
            (ItemKind::Break, "\n")
        };
        let written = self.push_leaf(NodeKind::LineBreak, kind, around, key, |writer, item| {
            let at = TextOffset::new(writer.content.text.len());
            writer.append(item, text);
            writer.content.record(|map| map.generated(0..1, at));
            if !in_ruby {
                writer.first_line.mirror = Mirror::Done;
                if let Some(clear) = clear {
                    let clearance = BreakClearance { item, clear };
                    writer
                        .content
                        .extras_mut()
                        .clearances
                        .push_bounded(clearance, "a clearance has an item, and fits where it does");
                }
            }
        });
        if !written {
            return;
        }
        self.text_node.last_char = '\n';
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
        if self.room.full {
            return;
        }
        if !self.has_text_room(ZWSP.len_utf8()) {
            self.room.full = true;
            return;
        }
        self.text_node.last_char = ZWSP;
        // How far the text node's text has got; between nodes, how far the
        // last node written got.
        let (node, source) = match self.text_node.node {
            Some(node) => (node, self.text_node.source),
            None => (self.container(), self.content.extras().map.source_end()),
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
        while !self.scratch.open.is_empty() {
            self.pop();
        }
        if self.trim.ends {
            self.trim_end();
        }
        let end = self.content.items.next_id();
        if let Some(block) = self.content.nodes.get_mut(NodeId::BLOCK) {
            block.end = end;
        }
        // The boxes are pushed in node order, and found by key.
        if let Some(extras) = self.content.extras.as_deref_mut() {
            extras.keyed_boxes.sort_from(0);
        }
        #[cfg(debug_assertions)]
        self.debug_check();
        #[cfg(debug_assertions)]
        check(self.content);
        self.cx.put_content_scratch(mem::take(&mut self.scratch));
        self.room.report
    }

    /// Returns the content written and the context, for the stages that
    /// read it once it is finished.
    pub(crate) fn parts(&mut self) -> (&Content, &mut Context) {
        (self.content, self.cx)
    }
}
