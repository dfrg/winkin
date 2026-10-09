//! Lowering each node's styles into facts, and the writer's reads of the
//! facts it lowered.

use alloc::boxed::Box;

#[cfg(any(debug_assertions, test))]
use super::facts_check;
use super::memo::StyleKey;
use super::transform::{TextTransformer, Transforms, case_langid};
use super::{
    BoxFactsId, Content, ContentFlags, ContentWriter, InitialLetterUse, LoweredNode, NodeFacts,
    NodeId, NodeKind, TextFacts, TextFactsId, TextFlags,
};
use crate::style::{
    BidiGroup, ComputedStyle, Direction, FirstLineVariant, RubyGroup, TextCase, TextTransform,
};

impl ContentWriter<'_> {
    /// Returns `style` keyed for the writer, its lists interned. Lists that
    /// do not fit are replaced and counted.
    pub(super) fn key(&mut self, style: &ComputedStyle<'_>) -> StyleKey {
        self.content
            .lists
            .key(style, &mut self.report.replaced_styles)
    }

    /// Lowers a node's style, and its pinned first-line style where the
    /// block has `::first-line`, into the facts a node of `kind` has.
    ///
    /// `initial-letter` is kept only for an inline box (`inline_box`) that
    /// may be the block's initial letter, which then takes it (see
    /// [`initial_letter_use`](Self::initial_letter_use)). Everything else is
    /// lowered with its used value, `normal`. Where a fact table cannot take
    /// the facts, the container's text facts and the initial box facts
    /// stand in.
    pub(super) fn lower_node_styles(
        &mut self,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        kind: NodeKind,
        inline_box: bool,
    ) -> LoweredNode {
        let letter = self.initial_letter_use(style, inline_box);
        let style = &letter.apply(style);
        let own = self.key(style);
        let first_line = self.first_line.then(|| match first_line {
            Some(first_line) => self.key(&letter.apply(&style.pinned_first_line(first_line))),
            None => own,
        });
        let around = self.container_facts();
        self.lower_both(&own, first_line.as_ref(), kind, Some(around))
    }

    /// Lowers the facts of a node of `kind` set in `own` and, where the
    /// block has `::first-line`, in `first_line`.
    ///
    /// Each variant is lowered by [`lower`](Self::lower), with the
    /// container's facts in that variant, `around`, as its stand-in. The
    /// block has no container, so it has none. The result also says how
    /// the text written in the node is transformed.
    pub(super) fn lower_both(
        &mut self,
        own: &StyleKey,
        first_line: Option<&StyleKey>,
        kind: NodeKind,
        around: Option<LoweredNode>,
    ) -> LoweredNode {
        let stand_in = around.map_or(TextFactsId::default(), |around| around.own.text);
        let lowered = self.lower(own, kind, stand_in);
        let first = first_line.map(|first| {
            // The block's first line stands in its own text facts.
            let stand_in = match around {
                Some(around) => around.first_line.unwrap_or(around.own).text,
                None => lowered.text,
            };
            if first == own {
                lowered
            } else {
                self.lower(first, kind, stand_in)
            }
        });
        LoweredNode {
            own: lowered,
            first_line: first,
            restyles: first_line.is_some_and(|first| first != own),
            transforms: self.transforms(own, first_line),
        }
    }

    /// Lowers `style` into the facts of a node of `kind`.
    ///
    /// The text facts come from the memo, or are lowered and held there.
    /// Each new row is interned, and the content's flags are set for what
    /// it holds. The block's styles are lowered once a build and not held,
    /// so a layout with no box makes no memo.
    ///
    /// Where a fact table is full, the node's text facts are `stand_in`,
    /// its parent's, since text facts inherit. Its box facts are the
    /// initial ones, a culled box's, never its parent's. Each counts as a
    /// style replaced.
    fn lower(&mut self, style: &StyleKey, kind: NodeKind, stand_in: TextFactsId) -> NodeFacts {
        let Content {
            lists,
            facts,
            block,
            flags,
            ..
        } = &mut *self.content;
        let writing_mode = block.writing_mode;
        let (languages, lookup) = lists.lowering();
        if matches!(style.text.transform.case, TextCase::MathAuto) {
            flags.insert(ContentFlags::MATH_AUTO);
        }
        let mut new = ContentFlags::NONE;
        let mut lower_text = || {
            let lowered = facts.lower_text(lookup, languages, style, writing_mode);
            new = lowered.flags;
            lowered.id
        };
        let text = match kind {
            NodeKind::Block => lower_text(),
            _ => self
                .memo
                .get_or_insert_with(Box::default)
                .text(style, lower_text),
        };
        let box_ = facts.lower_box(lookup, style, kind, writing_mode);
        flags.insert(new.union(box_.flags));
        #[cfg(any(debug_assertions, test))]
        facts_check::lowered(self.content, style, kind, text, box_.id);
        let text = text.unwrap_or_else(|| {
            self.report.replace_style();
            stand_in
        });
        let box_ = box_.id.unwrap_or_else(|| {
            self.report.replace_style();
            BoxFactsId::INITIAL
        });
        NodeFacts { text, box_ }
    }

    /// Returns the box facts of a box that keeps nothing of its own but its
    /// `direction`, as a plain first letter does. New rows are interned.
    pub(super) fn plain_box(&mut self, direction: Direction) -> BoxFactsId {
        self.box_keeping(direction, RubyGroup::INITIAL, NodeKind::FirstLetter)
    }

    /// Returns the box facts of an anonymous ruby container, which keeps
    /// nothing of its own but its `direction` and the ruby properties it
    /// inherits, `ruby`. New rows are interned.
    pub(super) fn anonymous_ruby_box(
        &mut self,
        direction: Direction,
        ruby: RubyGroup,
    ) -> BoxFactsId {
        self.box_keeping(direction, ruby, NodeKind::Ruby)
    }

    /// Returns the box facts of a node of `kind` that keeps only `direction`
    /// and `ruby`. New rows are interned.
    fn box_keeping(&mut self, direction: Direction, ruby: RubyGroup, kind: NodeKind) -> BoxFactsId {
        let style = StyleKey {
            bidi: BidiGroup {
                direction,
                ..BidiGroup::INITIAL
            },
            ruby,
            ..StyleKey::INITIAL
        };
        let Content {
            lists,
            facts,
            block,
            flags,
            ..
        } = &mut *self.content;
        let (_, lookup) = lists.lowering();
        let box_ = facts.lower_box(lookup, &style, kind, block.writing_mode);
        flags.insert(box_.flags);
        #[cfg(any(debug_assertions, test))]
        facts_check::lowered(self.content, &style, kind, None, box_.id);
        box_.id.unwrap_or_else(|| {
            self.report.replace_style();
            BoxFactsId::INITIAL
        })
    }

    /// Returns what `initial-letter` comes to on a box opening now, set in
    /// `style`.
    ///
    /// It is kept where the box is an inline box (`inline_box`) and a
    /// first child all the way up to the block, with only opening edges and
    /// float anchors before it, and no box is already the block's initial
    /// letter. This is the rule of CSS Inline 3, section 7.3.1. A box that
    /// keeps it here marks it taken. Otherwise its used value is `normal`.
    fn initial_letter_use(
        &mut self,
        style: &ComputedStyle<'_>,
        inline_box: bool,
    ) -> InitialLetterUse {
        if !style.line.initial_letter.is_set() {
            return InitialLetterUse::Unset;
        }
        if inline_box && self.may_open_initial_letter() {
            self.initial_letter = true;
            return InitialLetterUse::Kept;
        }
        InitialLetterUse::Normal
    }

    /// Whether a box opening now may be the block's initial letter. It may
    /// where only opening edges and float anchors have been written, and no
    /// box has taken it.
    pub(super) fn may_open_initial_letter(&self) -> bool {
        self.at_start && self.content.text.is_empty() && !self.initial_letter
    }

    /// Returns `node`'s own text facts.
    pub(super) fn text_facts(&self, node: NodeId) -> &TextFacts {
        let nodes = &self.content.nodes;
        self.content
            .facts
            .text(nodes.text_facts(node, FirstLineVariant::Standard))
    }

    /// Returns the direction of `node`'s text in `variant`, from its text
    /// facts.
    pub(super) fn direction(&self, node: NodeId, variant: FirstLineVariant) -> Direction {
        let text = self.content.nodes.text_facts(node, variant);
        if self.content.facts.text(text).has(TextFlags::RTL) {
            Direction::Rtl
        } else {
            Direction::Ltr
        }
    }

    /// How text set in `style` is transformed, or `None` where it is not.
    fn transformer(&self, style: &StyleKey) -> Option<TextTransformer> {
        let transform = style.text.transform;
        if transform == TextTransform::NONE {
            return None;
        }
        Some(TextTransformer::new(
            transform,
            case_langid(&self.content.lists.languages.get(style.text.language)),
            style.text.white_space_collapse.keeps_spaces(),
        ))
    }

    /// How the text written in a container set in `own` is transformed.
    /// On the first line, where `first_line` transforms it otherwise, that
    /// style's transform applies.
    fn transforms(&self, own: &StyleKey, first_line: Option<&StyleKey>) -> Transforms {
        let first_line = first_line
            .filter(|first| first.text.transform != own.text.transform)
            .map(|first| self.transformer(first));
        Transforms::new(self.transformer(own), first_line)
    }

    /// How the text written in the innermost container open, or the
    /// block, is transformed.
    pub(super) fn container_transforms(&self) -> Transforms {
        self.stack
            .last()
            .map_or(self.block_transforms, |open| open.transforms)
    }

    /// Returns the facts that text and a `<br>` written in the innermost
    /// container take: its text facts, and no box facts.
    ///
    /// The first-line ones come too, where the block has `::first-line`.
    /// Whether the container's first line restyles it was noted with the
    /// container.
    pub(super) fn container_facts(&self) -> LoweredNode {
        let container = self.container();
        let nodes = &self.content.nodes;
        let facts = |variant| NodeFacts {
            text: nodes.text_facts(container, variant),
            box_: BoxFactsId::INITIAL,
        };
        LoweredNode {
            own: facts(FirstLineVariant::Standard),
            first_line: self.first_line.then(|| facts(FirstLineVariant::FirstLine)),
            restyles: false,
            transforms: self.container_transforms(),
        }
    }

    /// Records whether a node's first-line style restyles anything of its
    /// own (`restyles`), and whether its text shapes otherwise on the first
    /// line (`first_line` against `own`).
    ///
    /// The flags for what each holds were set as it was lowered.
    pub(super) fn note_first_line(
        &mut self,
        own: NodeFacts,
        first_line: NodeFacts,
        restyles: bool,
    ) {
        if restyles {
            self.content.flags.insert(ContentFlags::FIRST_LINE_RESTYLE);
        }
        let facts = &self.content.facts;
        if facts.text(own.text).shaping != facts.text(first_line.text).shaping {
            self.content.flags.insert(ContentFlags::FIRST_LINE_RESHAPES);
        }
    }
}
