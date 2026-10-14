//! Lowers node styles into facts, and reads back the facts lowered.

use super::{ContentScratch, ContentWriter, InitialLetterUse, LoweredNode, NodeFacts};
#[cfg(any(debug_assertions, test))]
use crate::stages::content::facts_check;
use crate::stages::content::lists::Lookup;
use crate::stages::content::memo::StyleKey;
use crate::stages::content::transform::{TextTransformer, Transforms};
use crate::stages::content::{
    BoxFactsId, Content, ContentFlags, Facts, NodeId, NodeKind, TextFacts, TextFactsId, TextFlags,
};
use crate::unicode::CaseLanguage;

/// The fact tables, their lookup and the content's flags, which lowering
/// writes.
type Lowering<'a> = (&'a mut Facts, &'a mut Lookup, &'a mut ContentFlags);
use crate::style::{
    BidiGroup, ComputedStyle, Direction, FirstLineVariant, RubyGroup, TextTransform,
};

impl ContentWriter<'_> {
    /// Returns `style` with its lists interned. Lists that don't fit are
    /// replaced and counted.
    pub(super) fn key(&mut self, style: &ComputedStyle<'_>) -> StyleKey {
        let (fonts, ..) = self.cx.selecting_fonts();
        self.scratch.lists.key(
            style,
            &mut self.content.languages,
            fonts,
            &mut self.room.report.replaced_styles,
        )
    }

    /// Returns a node's style keyed with `letter` applied, and, when the
    /// block has `::first-line`, its pinned first-line style keyed the same
    /// way (the own key when `first_line` is `None`).
    pub(super) fn key_styles(
        &mut self,
        letter: InitialLetterUse,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) -> (StyleKey, Option<StyleKey>) {
        let style = &letter.apply(style);
        let own = self.key(style);
        let first_line = self.first_line.styled.then(|| match first_line {
            Some(first_line) => self.key(&letter.apply(&style.pinned_first_line(first_line))),
            None => own,
        });
        (own, first_line)
    }

    /// Lowers a node's style, and its first-line style if any, into facts
    /// for a node of `kind`.
    ///
    /// `initial-letter` is kept only for an `inline_box` that may be the
    /// initial letter (see [`initial_letter_use`](Self::initial_letter_use));
    /// elsewhere it is `normal`. If a fact table is full, the container's
    /// text facts and the initial box facts stand in.
    pub(super) fn lower_node_styles(
        &mut self,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        kind: NodeKind,
        inline_box: bool,
    ) -> LoweredNode {
        let letter = self.initial_letter_use(style, inline_box);
        let (own, first_line) = self.key_styles(letter, style, first_line);
        let around = self.container_facts();
        self.lower_both(&own, first_line.as_ref(), kind, Some(around))
    }

    /// Lowers keyed styles `own` and `first_line` into facts and transforms
    /// for a node of `kind`.
    ///
    /// Each variant uses the container's text facts in that variant,
    /// `around`, as its stand-in; the block has no container (`None`).
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
            // The block's first line falls back to its own text facts.
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
    /// Facts come from the memo, or are lowered, interned and memoized; new
    /// rows set the content's flags. The block's styles skip the memo.
    ///
    /// If a fact table is full, the text facts are `stand_in`, the
    /// parent's, since text facts inherit; the box facts are the initial
    /// ones, a culled box's. Each counts as a replaced style.
    fn lower(&mut self, style: &StyleKey, kind: NodeKind, stand_in: TextFactsId) -> NodeFacts {
        let basis = self.content.basis();
        let Content {
            languages,
            facts,
            block,
            flags,
            ..
        } = &mut *self.content;
        let writing_mode = block.writing_mode;
        let ContentScratch { memo, lists, .. } = &mut self.scratch;
        let languages = &*languages;
        let lookup = lists.lookup();
        if style.text.transform.is_math_auto() {
            flags.insert(ContentFlags::MATH_AUTO);
        }
        let lower_text = |(facts, lookup, flags): &mut Lowering<'_>| {
            let lowered = facts.lower_text(lookup, languages, style, writing_mode);
            flags.insert(lowered.flags);
            lowered.id
        };
        let lower_box = |(facts, lookup, flags): &mut Lowering<'_>| {
            let lowered = facts.lower_box(lookup, style, kind, writing_mode, basis);
            flags.insert(lowered.flags);
            lowered.id
        };
        let cx = &mut (facts, lookup, flags);
        let (text, box_) = match kind {
            NodeKind::Block => (lower_text(cx), lower_box(cx)),
            _ => memo.facts(cx, style, kind, lower_text, lower_box),
        };
        #[cfg(any(debug_assertions, test))]
        facts_check::lowered(self.content, style, kind, text, box_);
        let text = text.unwrap_or_else(|| {
            self.room.report.replace_style();
            stand_in
        });
        let box_ = box_.unwrap_or_else(|| {
            self.room.report.replace_style();
            BoxFactsId::INITIAL
        });
        NodeFacts { text, box_ }
    }

    /// Returns the box facts of a plain first letter, which keeps only its
    /// `direction`.
    pub(super) fn plain_box(&mut self, direction: Direction) -> BoxFactsId {
        self.box_keeping(direction, RubyGroup::INITIAL, NodeKind::FirstLetter)
    }

    /// Returns the box facts of an anonymous ruby container, which keeps
    /// only its `direction` and inherited `ruby` properties.
    pub(super) fn anonymous_ruby_box(
        &mut self,
        direction: Direction,
        ruby: RubyGroup,
    ) -> BoxFactsId {
        self.box_keeping(direction, ruby, NodeKind::Ruby)
    }

    /// Returns the box facts of a node of `kind` that keeps only `direction`
    /// and `ruby`, interning a new row if needed.
    fn box_keeping(&mut self, direction: Direction, ruby: RubyGroup, kind: NodeKind) -> BoxFactsId {
        let style = StyleKey {
            bidi: BidiGroup {
                direction,
                ..BidiGroup::INITIAL
            },
            ruby,
            ..StyleKey::INITIAL
        };
        let basis = self.content.basis();
        let Content {
            facts,
            block,
            flags,
            ..
        } = &mut *self.content;
        let lookup = self.scratch.lists.lookup();
        let box_ = facts.lower_box(lookup, &style, kind, block.writing_mode, basis);
        flags.insert(box_.flags);
        #[cfg(any(debug_assertions, test))]
        facts_check::lowered(self.content, &style, kind, None, box_.id);
        box_.id.unwrap_or_else(|| {
            self.room.report.replace_style();
            BoxFactsId::INITIAL
        })
    }

    /// Returns the used `initial-letter` for a box opening now in `style`.
    ///
    /// It is kept for an `inline_box` that
    /// [`may_open_initial_letter`](Self::may_open_initial_letter) (CSS
    /// Inline 3, section 7.3.1), which then takes the initial letter.
    /// Otherwise it is `normal`.
    fn initial_letter_use(
        &mut self,
        style: &ComputedStyle<'_>,
        inline_box: bool,
    ) -> InitialLetterUse {
        if !style.line.initial_letter.is_set() {
            return InitialLetterUse::Unset;
        }
        if inline_box && self.may_open_initial_letter() {
            self.letters.initial_letter = true;
            return InitialLetterUse::Kept;
        }
        InitialLetterUse::Normal
    }

    /// Whether a box opening now may be the initial letter: no text and no
    /// items but opening edges and float or absolute anchors have been
    /// written, and no box has taken it.
    pub(super) fn may_open_initial_letter(&self) -> bool {
        self.letters.at_start && self.content.text.is_empty() && !self.letters.initial_letter
    }

    /// Returns `node`'s own text facts.
    pub(super) fn text_facts(&self, node: NodeId) -> &TextFacts {
        let nodes = &self.content.nodes;
        self.content
            .facts
            .text(nodes.text_facts(node, FirstLineVariant::Standard))
    }

    /// Returns the direction of `node`'s text in `variant`.
    pub(super) fn direction(&self, node: NodeId, variant: FirstLineVariant) -> Direction {
        let text = self.content.nodes.text_facts(node, variant);
        if self.content.facts.text(text).has(TextFlags::RTL) {
            Direction::Rtl
        } else {
            Direction::Ltr
        }
    }

    /// Returns the transformer for `style`, or `None` for no transform.
    fn transformer(&self, style: &StyleKey) -> Option<TextTransformer> {
        let transform = style.text.transform;
        if transform == TextTransform::NONE {
            return None;
        }
        Some(TextTransformer::new(
            transform,
            CaseLanguage::from_language(self.content.languages.get(style.text.language).language()),
            style.text.white_space_collapse.keeps_spaces(),
        ))
    }

    /// Returns the transforms for text in a node styled `own`, with the
    /// first-line transform where `first_line` differs. The mask is `own`'s
    /// on every line.
    fn transforms(&self, own: &StyleKey, first_line: Option<&StyleKey>) -> Transforms {
        let first_line = first_line
            .filter(|first| first.text.transform != own.text.transform)
            .map(|first| self.transformer(first));
        Transforms::new(self.transformer(own), first_line).with_mask(own.text.security.mask())
    }

    /// Returns the transforms of the innermost open container, or the
    /// block's.
    pub(super) fn container_transforms(&self) -> Transforms {
        self.scratch
            .open
            .last()
            .map_or(self.block_transforms, |open| open.transforms)
    }

    /// Returns the facts for text and `<br>` in the innermost container:
    /// its text facts in both variants and the initial box facts.
    ///
    /// `restyles` is false; the container's own node already recorded it.
    pub(super) fn container_facts(&self) -> LoweredNode {
        let container = self.container();
        let nodes = &self.content.nodes;
        let facts = |variant| NodeFacts {
            text: nodes.text_facts(container, variant),
            box_: BoxFactsId::INITIAL,
        };
        LoweredNode {
            own: facts(FirstLineVariant::Standard),
            first_line: self
                .first_line
                .styled
                .then(|| facts(FirstLineVariant::FirstLine)),
            restyles: false,
            transforms: self.container_transforms(),
        }
    }

    /// Flags the content when a node's first-line style `restyles` it, or
    /// when its first-line text facts shape differently from `own`.
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
            self.content.flags.insert(ContentFlags::FIRST_LINE_SHAPING);
        }
    }
}
