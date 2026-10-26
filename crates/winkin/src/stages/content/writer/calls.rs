//! Builder calls other than text: containers, atomic inlines, floats,
//! absolutely positioned boxes, breaks and `finish`.

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
    Absolute, Atomic, AtomicId, BreakClearance, Content, ContentFlags, Float, ItemFlags, ItemKind,
    MAX_RUBY_DEPTH, NodeId, NodeKey, NodeKind,
};
use crate::style::{ComputedStyle, FirstLineVariant, RubyGroup, RubyPosition, WhiteSpaceTrim};

impl ContentWriter<'_> {
    /// Opens a container styled `style`, and `first_line` on the first line.
    ///
    /// A ruby container inside an annotation or past `MAX_RUBY_DEPTH` is
    /// flattened to a box.
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

    /// Opens an anonymous ruby container for annotation `key`, which has no
    /// ruby around it.
    ///
    /// Such a `ruby-text` box is wrapped in a ruby column with an empty
    /// base. The container takes the enclosing container's text facts and
    /// direction and the annotation's `ruby` properties, and closes with the
    /// annotation.
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

    /// Opens a container, its facts lowered by `lower`. If it doesn't fit, a
    /// dropped entry still goes on the stack to take its close.
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
        // No first letter in ruby, where CSS leaves an initial letter
        // undefined (CSS Inline 3, section 7.3.1).
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
        // Room for the node, its opening item and its close.
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

    /// Closes the innermost container if it is a box; otherwise does
    /// nothing.
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

    /// Closes the innermost ruby container, nested ones included.
    ///
    /// A flattened container consumes its `end_ruby` even if an annotation
    /// already closed its box.
    pub(crate) fn end_ruby(&mut self) {
        if self.ruby.flattened > 0 {
            self.ruby.flattened -= 1;
            self.close_through(ContainerKind::FlattenedRuby, Some(ContainerKind::Ruby));
            return;
        }
        self.close_through(ContainerKind::Ruby, None);
    }

    /// Closes containers through the innermost of `kind`; does nothing if
    /// none is open.
    ///
    /// The search follows the ruby chain, skipping boxes, and does not look
    /// past a `stop` kind, so an annotation ends only within the innermost
    /// ruby. A call that closes nothing looks at an entry or two at any
    /// box depth.
    fn close_through(&mut self, kind: ContainerKind, stop: Option<ContainerKind>) {
        // A flattened ruby still stops a search that stops at ruby.
        let stops = |open: &Open| {
            Some(open.kind) == stop
                || (stop == Some(ContainerKind::Ruby) && open.kind == ContainerKind::FlattenedRuby)
        };
        let found = self
            .ruby_chain()
            .find(|(_, open)| open.kind == kind || stops(open))
            .filter(|(_, open)| open.kind == kind);
        // Close everything inside it, then it.
        if let Some((id, _)) = found {
            while id < self.scratch.open.next_id() {
                self.pop();
            }
        }
    }

    /// Opens an annotation in the innermost real ruby container, first
    /// closing whatever is open inside it (the previous annotation, a
    /// flattened ruby). Outside any ruby, or inside a box in a base, it opens
    /// in an anonymous ruby container, as CSS Ruby wraps an annotation whose
    /// parent is no ruby container (CSS Ruby 1, section 2.2).
    ///
    /// Its `ruby-position` is `side` if given, else that of the innermost
    /// ruby container, flattened or not, read before it closes.
    pub(crate) fn annotation(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        side: Option<RubyPosition>,
    ) {
        // Nothing written since `start_column` asked for an empty base.
        let empty_base = self.ruby.empty_base.take() == Some(self.content.items.next_id());
        // In a box in a base, where a nested container still fits.
        let in_base_box = self.ruby.annotations == 0
            && self.ruby.depth < MAX_RUBY_DEPTH
            && (self.scratch.open.last()).is_some_and(|open| open.kind == ContainerKind::Box);
        let found = match self.annotated_ruby() {
            Some(found) if !in_base_box => Some(found),
            _ => {
                self.open_anonymous_ruby(key, style);
                self.annotated_ruby()
            }
        };
        let Some((ruby, container)) = found else {
            return;
        };
        let nodes = &self.content.nodes;
        let container_side = |variant| {
            side.unwrap_or_else(|| {
                self.content
                    .facts
                    .box_facts(nodes.box_facts(container.container, variant))
                    .ruby
                    .position
            })
        };
        let position = container_side(FirstLineVariant::Standard);
        // `::first-line` may change the container's side.
        let first_position = container_side(FirstLineVariant::FirstLine);
        // Close everything inside the ruby container.
        while self
            .scratch
            .open
            .ids()
            .next_back()
            .is_some_and(|innermost| innermost > ruby)
        {
            self.pop();
        }
        fn sided<'s>(style: &ComputedStyle<'s>, position: RubyPosition) -> ComputedStyle<'s> {
            ComputedStyle {
                ruby: RubyGroup {
                    position,
                    ..style.ruby
                },
                ..*style
            }
        }
        let first_line = match first_line {
            Some(first_line) => Some(sided(first_line, first_position)),
            None if first_position != position => Some(sided(style, first_position)),
            None => None,
        };
        let style = sided(style, position);
        self.open(ContainerKind::Annotation, key, &style, first_line.as_ref());
        if empty_base
            && let Some(open) = self.scratch.open.last()
            && open.kind == ContainerKind::Annotation
            && open.node.is_some()
            && let Some(item) = self.content.items.get_mut(open.item)
        {
            item.flags.insert(ItemFlags::EMPTY_BASE);
        }
    }

    /// Gives the next annotation an empty base of its own where the
    /// innermost open container is a ruby container and no base text has
    /// come since its last annotation closed.
    ///
    /// If nothing else is written first, that annotation's opening item
    /// carries `EMPTY_BASE`, and later stages start a column there.
    pub(crate) fn start_column(&mut self) {
        self.end_text();
        let Some(ruby) = self.scratch.open.last() else {
            return;
        };
        if ruby.kind != ContainerKind::Ruby || ruby.node.is_none() {
            return;
        }
        let close = ruby
            .annotation_close
            .and_then(|close| self.content.items.get(close));
        if close.is_some_and(|close| close.end.get() == self.content.text.len()) {
            self.ruby.empty_base = Some(self.content.items.next_id());
        }
    }

    /// Returns the stack id of the real ruby container a new annotation
    /// opens in, and the innermost ruby container, real or flattened, whose
    /// side it takes. Both are found along the ruby chain.
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

    /// Closes the innermost annotation of the innermost ruby container, with
    /// everything inside it. An anonymous container closes with it.
    pub(crate) fn end_annotation(&mut self) {
        self.close_through(ContainerKind::Annotation, Some(ContainerKind::Ruby));
        if self.scratch.open.last().is_some_and(|open| open.anonymous) {
            self.pop();
        }
    }

    /// Writes an atomic inline: one U+FFFC in its own node.
    pub(crate) fn atomic(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        size: BoxSize,
    ) {
        self.end_text();
        // Room for U+FFFC and a pending run's space.
        if !self.has_node_room() || !self.has_item_room(1) || !self.has_text_room(OBJECT.len() + 1)
        {
            self.room.report.drop_node();
            return;
        }
        let lowered = self.lower_node_styles(style, first_line, NodeKind::Atomic, false);
        // An atomic inline before the first letter rules it out (CSS
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
                // Source offsets 0 and 1 of its node, before and after it.
                let at = TextOffset::new(writer.content.text.len());
                writer.append(item, OBJECT);
                writer.content.record(|map| map.generated(0..1, at));
                let writing_mode = writer.content.block.writing_mode;
                let basis = writer.content.basis();
                if style.edges.margin.has_percentage() {
                    writer.content.flags.insert(ContentFlags::PERCENTAGES);
                }
                let calc = writer.cx.calc_resolver();
                let atomic = Atomic::new(item, size, style, writing_mode, basis, calc);
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

    /// Records `atomic`'s `key`, and flags the content if an earlier atomic
    /// inline has the same key.
    ///
    /// After the flag is set it records nothing more; searches by key read
    /// only the flag.
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

    /// Writes a float's anchor, which has no text and is opaque to
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
        // `::first-line` styles only inline content.
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
                let calc = writer.cx.calc_resolver();
                let float = Float::new(item, size, side, style, writing_mode, basis, calc);
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

    /// Writes an absolutely positioned box's anchor, in a node with its
    /// container's facts.
    ///
    /// Like a float's, it has no text and is opaque to collapsing, as Blink's
    /// `AppendOutOfFlowPositioned` writes it. Unlike a float, capitalize
    /// reads past it.
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

    /// Writes a forced break: one `\n` in its own node, with its
    /// container's facts.
    ///
    /// It removes collapsible white space on both sides, ends the first
    /// line and rules out a first letter still to come. `clear` is recorded
    /// for a break that clears floats.
    ///
    /// Inside ruby it is a space instead (Blink's
    /// `kDisableForcedBreakInRubyColumn`), since no paragraph ends inside a
    /// column. The space ends the run before it and absorbs the white space
    /// after it.
    pub(crate) fn line_break(&mut self, key: NodeKey, clear: Option<Clear>) {
        let in_ruby = self.ruby.innermost.is_some();
        self.end_text();
        // Room for its character, and in ruby a pending run's space.
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
            // White space after it collapses away, as after a forced break.
            self.collapser.step(Event::ForcedBreak);
        }
    }

    /// Writes a `<wbr>` as a generated U+200B, in the current text node or
    /// else the enclosing container.
    ///
    /// It is opaque to collapsing. It ends the text node `math-auto` is
    /// measuring, as an element between two text nodes does.
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
        // The source position: in the text node, or after the last node.
        let (node, source) = match self.text_node.node {
            Some(node) => (node, self.text_node.source),
            None => (self.container(), self.content.extras().map.source_end()),
        };
        self.generated_break(node, source);
    }

    /// Closes everything open, settles the block's trailing white space,
    /// returns the working memory to the context, and reports what was
    /// dropped.
    ///
    /// The builder calls this once, when it finishes or is dropped; the
    /// content is frozen after.
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
        // Keyed boxes were pushed in node order; sort them for lookup by key.
        if let Some(extras) = self.content.extras.as_deref_mut() {
            extras.keyed_boxes.sort_from(0);
        }
        #[cfg(debug_assertions)]
        self.debug_check();
        #[cfg(debug_assertions)]
        check(self.content);
        self.scratch.staging.settle(self.content);
        self.cx.put_content_scratch(mem::take(&mut self.scratch));
        self.room.report
    }

    /// Returns the finished content and the context, for the stages that
    /// read it.
    pub(crate) fn parts(&mut self) -> (&Content, &mut Context) {
        (self.content, self.cx)
    }
}
