//! Paint operations and geometry in Chrome paint order.
//!
//! [`Line::paints`] yields [`Paint`] values. The host draws them using colors,
//! backgrounds, borders and decoration styles retrieved by [`NodeKey`].
//! Operations follow CSS 2.1 Appendix E:
//!
//! 1. The block background behind the line content.
//! 2. Retained box fragments, outermost first in tree order.
//! 3. Underlines and overlines: block first, then retained and culled boxes.
//! 4. Ruby boxes and text, with emphasis marks after each run.
//! 5. Text and atomic inlines in visual order, with emphasis marks after each run.
//! 6. The generated hyphen. Ellipses appear with text at their visual position.
//! 7. Line-through decorations.
//!
//! Decorations span runs of text within the decorating box. Box edges,
//! objects, unrelated text and hanging whitespace split bars; color changes
//! do not. Initial letters are objects to enclosing decorations, but retain
//! their own and descendant decorations.
//!
//! Each bar provides reference baseline and font metrics, Chrome-compatible
//! underline helpers, and font underline/strikeout metrics for `from-font`.
//! Culled boxes can decorate text without retaining fragments. Their bars use
//! the parent baseline; block decorations use the line baseline.
//!
//! Only culled boxes marked by [`ComputedStyle::decorates`](crate::ComputedStyle::decorates)
//! are queried for decorations. Retained boxes are always queried.
//! Reading paint operations does not allocate.

use core::fmt;
use core::ops::Range;
use core::slice::Iter;

pub use super::emphasis::EmphasisMark;

use super::emphasis::EmphasisMarks;
use super::{BoxFragment, CrossExtents, Line, TextRun};
use crate::data::{Id, Table};
use crate::stages::analysis::{ClusterId, ParagraphFlags};
use crate::stages::content::{
    BoxFlags, Content, ContentFlags, ItemFlags, ItemId, ItemKind, NodeId, NodeKey, NodeKind,
};
use crate::stages::fonts::Generated;
use crate::stages::fragments::{FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind};
use crate::stages::lines::InlineExtents;
use crate::stages::measure::{KeptBoxId, KeptBoxes};
use crate::style::FirstLineVariant;
use crate::unit::{self, InlineLayoutUnit, LayoutUnit};

use super::boxes::Atomic;
use crate::work;

/// Decoration lines grouped by paint phase.
///
/// CSS 2.1 Appendix E paints underlines and overlines before text and
/// line-through after text. [`Line::paints`] queries the host-supplied
/// style for the block and each box.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Decorates {
    /// No decoration line.
    #[default]
    None,
    /// Lines painted before the text: an underline, an overline, or both.
    BeforeText,
    /// A line painted after the text: a line through it.
    AfterText,
    /// Lines both before and after the text.
    Both,
}

impl Decorates {
    /// Whether it draws a line painted before the text.
    fn before_text(self) -> bool {
        matches!(self, Self::BeforeText | Self::Both)
    }

    /// Whether it draws a line painted after the text.
    fn after_text(self) -> bool {
        matches!(self, Self::AfterText | Self::Both)
    }
}

/// A paint operation in drawing order.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Paint<'a> {
    /// The block's background behind the line's content.
    Background(Background),
    /// The border box of a retained inline-box fragment.
    ///
    /// Culled boxes are available through
    /// [`Layout::box_fragments`](crate::Layout::box_fragments).
    Box(BoxFragment<'a>),
    /// A decoration line painted before the text: an underline or overline.
    ///
    /// An underline or overline for a run of box or block text.
    DecorationBeforeText(Decoration),
    /// A run of a ruby annotation's text, on its annotation's baseline.
    ///
    /// See [`Line::annotations`].
    Annotation(TextRun<'a>),
    /// A run of text.
    Text(TextRun<'a>),
    /// An emphasis mark, after the run whose text it marks.
    ///
    /// See [`TextRun::emphasis_marks`].
    Emphasis(EmphasisMark),
    /// An atomic inline, which the host paints.
    Atomic(Atomic<'a>),
    /// Text the layout made: a hyphen or an ellipsis.
    ///
    /// A hyphen or an ellipsis follows the text of its line.
    Generated(TextRun<'a>),
    /// A decoration line painted after the text: a line through it.
    ///
    /// A line-through for a run of box or block text.
    DecorationAfterText(Decoration),
}

/// The block background behind one line of content.
///
/// Spans the line-box height and content width, excluding hanging content.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Background {
    inline: InlineExtents,
    block: CrossExtents,
}

impl Background {
    /// Returns inline extents relative to line-box left.
    pub fn inline(&self) -> InlineExtents {
        self.inline
    }

    /// Returns cross-line extents relative to the line-over edge.
    pub fn block(&self) -> CrossExtents {
        self.block
    }
}

/// A decoration bar with its position and reference font metrics.
#[derive(Copy, Clone)]
pub struct Decoration {
    key: NodeKey,
    inline: InlineExtents,
    baseline: f32,
    size: f32,
    ascent: f32,
    descent: f32,
    underline: Option<FontDecorationLine>,
    line_through: Option<FontDecorationLine>,
    /// The first and one past the last of its line's text items it runs
    /// under, which [`runs`](Self::runs) reads.
    first: FragmentItemId,
    end: FragmentItemId,
}

impl PartialEq for Decoration {
    /// Compares what a bar draws, not which items it was found over.
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.inline == other.inline
            && self.baseline == other.baseline
            && self.size == other.size
            && self.ascent == other.ascent
            && self.descent == other.descent
            && self.underline == other.underline
            && self.line_through == other.line_through
    }
}

impl fmt::Debug for Decoration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoration")
            .field("key", &self.key)
            .field("inline", &self.inline)
            .field("baseline", &self.baseline)
            .field("size", &self.size)
            .field("ascent", &self.ascent)
            .field("descent", &self.descent)
            .field("underline", &self.underline)
            .field("line_through", &self.line_through)
            .finish()
    }
}

/// A font decoration offset and thickness at the used primary-font size.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FontDecorationLine {
    /// The offset of the top edge below the baseline; negative above it.
    pub offset: f32,
    /// The thickness in pixels, at least one.
    pub thickness: f32,
}

impl Decoration {
    /// Returns the decorating box or block node key.
    pub fn key(&self) -> NodeKey {
        self.key
    }

    /// Returns inline extents relative to line-box left.
    pub fn inline(&self) -> InlineExtents {
        self.inline
    }

    /// Returns the reference baseline offset from line-box top.
    pub fn baseline(&self) -> f32 {
        self.baseline
    }

    /// Returns the used primary-font size, the basis for decoration thickness.
    pub fn font_size(&self) -> f32 {
        self.size
    }

    /// Returns the primary-font ascent above the baseline.
    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    /// Returns the primary-font descent below the baseline.
    pub fn descent(&self) -> f32 {
        self.descent
    }

    /// Returns the Chrome-compatible underline thickness.
    ///
    /// One tenth of the font size, rounded down, with a minimum of one pixel.
    pub fn underline_thickness(&self) -> f32 {
        unit::whole_to_f32(self.thickness())
    }

    /// Returns the offset below the baseline for an `auto` underline.
    ///
    /// Half the thickness, rounded up, with a minimum of one pixel.
    pub fn underline_gap(&self) -> f32 {
        let thickness = self.thickness();
        unit::whole_to_f32((thickness / 2 + thickness % 2).max(1))
    }

    /// Returns underline metrics from `post`, if available.
    ///
    /// The offset is rounded to whole pixels.
    pub fn font_underline(&self) -> Option<FontDecorationLine> {
        self.underline
    }

    /// Returns strikeout metrics from `OS/2`, if available.
    ///
    /// Chrome instead centers line-through one third of the ascent above
    /// the baseline and uses underline thickness.
    pub fn font_line_through(&self) -> Option<FontDecorationLine> {
        self.line_through
    }

    /// Returns thickness for `text-decoration-thickness: from-font`.
    ///
    /// Uses the font underline thickness, falling back to
    /// [`underline_thickness`](Self::underline_thickness), matching Chrome.
    pub fn underline_thickness_from_font(&self) -> f32 {
        self.underline
            .map_or_else(|| self.underline_thickness(), |line| line.thickness)
    }

    /// Returns the underline offset for `text-underline-position: from-font`.
    ///
    /// Uses the font underline offset, falling back to
    /// [`underline_gap`](Self::underline_gap). Chrome applies this only to
    /// text on the alphabetic baseline.
    pub fn underline_gap_from_font(&self) -> f32 {
        self.underline
            .map_or_else(|| self.underline_gap(), |line| line.offset)
    }

    /// The thickness in whole pixels, worked in integers because a float
    /// has no rounding of its own without std.
    fn thickness(&self) -> i32 {
        unit::floor(self.size / 10.0).max(1)
    }

    /// Returns the text runs of `line`, the line it was painted on, that it
    /// was found over, in visual order.
    ///
    /// They are the runs it decorates, and any that take no room among
    /// them.
    pub(crate) fn runs<'a>(&self, line: Line<'a>) -> DecorationRuns<'a> {
        let items = line
            .layout()
            .fragments()
            .items
            .get_slice(self.first..self.end);
        DecorationRuns {
            line,
            items: items.unwrap_or_default().iter(),
        }
    }
}

/// The text runs a decoration bar was found over, in visual order: what
/// [`Decoration::runs`] walks.
pub(crate) struct DecorationRuns<'a> {
    line: Line<'a>,
    items: Iter<'a, FragmentItem>,
}

impl<'a> Iterator for DecorationRuns<'a> {
    type Item = TextRun<'a>;

    fn next(&mut self) -> Option<TextRun<'a>> {
        let item = self.items.by_ref().find(|item| {
            item.kind() == FragmentItemKind::Text && !item.flags.contains(FragmentItemFlags::HIDDEN)
        })?;
        Some(TextRun::new(self.line, item))
    }
}

/// The phase a line's paint is in, with where it has got to.
#[derive(Clone, Debug)]
enum Phase {
    /// The block's background behind the line.
    Background,
    /// The box parts, as line layout linked them: the next to paint, `None`
    /// past the last.
    Boxes(Option<FragmentItemId>),
    /// Underlines and overlines.
    BeforeText(Decorating),
    /// The annotations' items: the next to look at.
    Annotations(FragmentItemId),
    /// The line's leaves, from the next to look at, with the items of the
    /// hyphen the line owes, which the text phase passes and the hyphen
    /// phase paints after the text: one a font, side by side. Empty until
    /// the text phase meets them.
    Text {
        at: FragmentItemId,
        hyphen: Range<FragmentItemId>,
    },
    /// The hyphen's items still to paint.
    Hyphen(Range<FragmentItemId>),
    /// Lines through the text.
    AfterText(Decorating),
    /// Nothing more.
    Done,
}

/// Where a decoration phase has got to: the node it decorates and where
/// that node's next bar is looked for.
#[derive(Copy, Clone, Debug)]
struct Decorating {
    /// Whether the phase paints after the text.
    after_text: bool,
    /// The node being decorated, in tree order: `None` before the first,
    /// which is the block.
    node: Option<NodeId>,
    /// Where among the line's items the next bar is looked for from.
    at: FragmentItemId,
    /// The first part of the next box to decorate: `None` past the last.
    next: Option<FragmentItemId>,
    /// Where the bars of the node being decorated end: one past the last
    /// item of its last part, the line's end for the block.
    end: FragmentItemId,
    /// The baseline of the node being decorated: its parts', or the
    /// line's for the block.
    baseline: LayoutUnit,
    /// Where the phase's walk over the line's culled boxes has got to, once
    /// the kept boxes are done.
    culled: Option<CulledBoxes>,
    /// Where the bars of the culled box being decorated end, where the line
    /// sets its leaves in the order of their clusters: the cluster its text
    /// ends at.
    until: Option<ClusterId>,
}

impl Decorating {
    /// Returns the start of the phase painted after the text where
    /// `after_text`, and before it otherwise.
    #[inline]
    fn new(own: &Range<FragmentItemId>, after_text: bool) -> Self {
        Self {
            after_text,
            node: None,
            at: own.start,
            next: None,
            end: own.end,
            baseline: LayoutUnit::ZERO,
            culled: None,
            until: None,
        }
    }
}

/// A line's paint, in Chrome's order: what [`Line::paints`] walks.
///
/// **Linear in the line's items**, however deep its boxes nest or the
/// reordering splits them. Line layout links each box item to the one painted
/// after it (by box in tree order, a box's parts left to right). So the
/// boxes are walked by following the links, and each decorating box's bars
/// are looked for among its own parts' items only. A line's decorations
/// cost the items each decorating box holds, as Chrome's propagated
/// decorations cost each text fragment every decorating box around it.
pub(super) struct PaintWalk<'a, F> {
    /// The line and the items its phases read.
    painted: PaintedLine<'a>,
    decorates: F,
    /// The emphasis marks of the run painted last, still to hand out.
    marks: Option<EmphasisMarks<'a>>,
    phase: Phase,
    /// What both decoration phases read of the line's culled boxes, found
    /// by the first phase that looks for one.
    culled: Option<CulledLine<'a>>,
}

/// The line a paint walks, and the items its phases read.
struct PaintedLine<'a> {
    line: Line<'a>,
    /// Every item of the layout, of which the phases read the line's.
    items: &'a Table<FragmentItemId, FragmentItem>,
    /// The line's own items.
    own: Range<FragmentItemId>,
    /// The line's annotation lines, each heading its text.
    annotations: Range<FragmentItemId>,
    /// The line's box item painted first, where it has one: the head of
    /// the links its box items are painted by.
    first: Option<FragmentItemId>,
}

/// Where a decoration phase's walk over a line's culled boxes stands: how
/// many of the decorating ones open across the line's start are still to
/// hand out, outermost first, and then the next item of the line to look
/// for a culled box opening at.
#[derive(Copy, Clone, Debug)]
struct CulledBoxes {
    /// How many of the boxes open across the line's start that draw are
    /// still to hand out: the outermost of them next.
    above: usize,
    /// The next item that may open a box on the line.
    item: ItemId,
    /// The first leaf of the last box opening on the line, where the next
    /// one's bars are looked for from.
    resume: FragmentItemId,
}

/// What both decoration phases read of a line's culled boxes: where they
/// start, found once, and what says which of them draw.
struct CulledLine<'a> {
    /// What says which boxes draw, its walk over the kept boxes carried
    /// from the phase before the text to the one after it.
    drawing: CulledDrawing<'a>,
    /// The node of the item holding the line's first cluster, which the
    /// boxes open across its start are around.
    start: NodeId,
    /// How many of those draw before the text, and how many after it.
    above_before: usize,
    above_after: usize,
    /// The first item after the start's, which may open a box on the line.
    item: ItemId,
    /// The line sets its leaves left to right in the order of their
    /// clusters: every cluster of the text is at its paragraph's level,
    /// which reads left to right. A culled box's bars are then looked for
    /// from the first leaf of the box handed out before it, which opens
    /// before it, to where its text ends.
    in_order: bool,
}

/// What says whether a culled box draws in a decoration phase: the line's
/// tables, and a walk over its kept boxes.
struct CulledDrawing<'a> {
    content: &'a Content,
    kept: &'a KeptBoxes,
    /// Where the walk over `kept` stands, moved to each box asked.
    at: KeptBoxId,
    variant: FirstLineVariant,
}

impl CulledDrawing<'_> {
    /// Returns the lines `node` draws where it is a culled box that
    /// decorates, as `decorates` says, and `Decorates::None` otherwise.
    ///
    /// Only a box that decorates is looked for among the kept boxes, by the
    /// walk, which the boxes asked keep near one another.
    fn sides(&mut self, node: NodeId, decorates: &dyn Fn(NodeKey) -> Decorates) -> Decorates {
        let (nodes, facts) = (&self.content.nodes, &self.content.facts);
        let decorating = nodes.kind(node).is_some_and(NodeKind::is_inline_box)
            && node != NodeId::BLOCK
            && facts
                .box_facts(nodes.box_facts(node, self.variant))
                .has(BoxFlags::DECORATES);
        if !decorating {
            return Decorates::None;
        }
        self.at = self.kept.walk_to(self.at, node);
        if self.kept.node(self.at) == Some(node) {
            return Decorates::None;
        }
        decorates(nodes.key(node))
    }

    /// Returns whether `node` is a culled box that decorates, and draws in
    /// the phase painted after the text where `after_text`, before it
    /// otherwise.
    fn draws(
        &mut self,
        node: NodeId,
        decorates: &dyn Fn(NodeKey) -> Decorates,
        after_text: bool,
    ) -> bool {
        let sides = self.sides(node, decorates);
        if after_text {
            sides.after_text()
        } else {
            sides.before_text()
        }
    }

    /// Returns the `k`th of the boxes around `start` that draw in the phase,
    /// the innermost first, or `None` where there are fewer.
    fn around(
        &mut self,
        start: NodeId,
        k: usize,
        decorates: &dyn Fn(NodeKey) -> Decorates,
        after_text: bool,
    ) -> Option<NodeId> {
        let mut count = 0;
        self.walk_up(start, |drawing, node| {
            if drawing.draws(node, decorates, after_text) {
                count += 1;
            }
            count == k
        })
    }

    /// Returns how many of the boxes around `start` draw before the text,
    /// and how many after it, in one walk up.
    fn count_around(
        &mut self,
        start: NodeId,
        decorates: &dyn Fn(NodeKey) -> Decorates,
    ) -> (usize, usize) {
        let (mut before, mut after) = (0, 0);
        self.walk_up(start, |drawing, node| {
            let sides = drawing.sides(node, decorates);
            before += usize::from(sides.before_text());
            after += usize::from(sides.after_text());
            false
        });
        (before, after)
    }

    /// Walks from `start` up through its parents, to the first `stop`
    /// says to stop at, and returns that one; `None` past the root.
    ///
    /// It walks up at most one step a node, whatever the parents say.
    fn walk_up(
        &mut self,
        start: NodeId,
        mut stop: impl FnMut(&mut Self, NodeId) -> bool,
    ) -> Option<NodeId> {
        let nodes = &self.content.nodes;
        let mut node = start;
        for _ in 0..nodes.len() {
            work::step();
            if stop(self, node) {
                return Some(node);
            }
            let parent = nodes.parent(node);
            if parent == node {
                break;
            }
            node = parent;
        }
        None
    }
}

impl<'a, F: Fn(NodeKey) -> Decorates> PaintWalk<'a, F> {
    /// What `line` paints, asking `decorates` which decoration lines each key draws.
    pub(super) fn new(line: &Line<'a>, decorates: F) -> Self {
        let fragments = line.layout().fragments();
        Self {
            painted: PaintedLine {
                line: *line,
                items: &fragments.items,
                own: fragments.line_item_ids(line.id()),
                annotations: fragments.annotation_ids(line.id()),
                first: line.head().and_then(FragmentItem::painted_next),
            },
            decorates,
            marks: None,
            phase: Phase::Background,
            culled: None,
        }
    }
}

// The walk is generic, so it is built in the crate that calls it, which
// inlines only what is marked `#[inline]`: each step it takes is. The
// culled boxes' steps ask `decorates` through a `&dyn Fn` and are built
// once, here. `next_decoration` stays generic: asking through a `&dyn Fn`
// for the block and every kept box slows a boxed article's paint by 2–3%.
impl<'a> PaintedLine<'a> {
    /// Whether `item` is a leaf that is painted, and not what hangs.
    #[inline]
    fn content(item: &FragmentItem) -> bool {
        matches!(
            item.kind(),
            FragmentItemKind::Text | FragmentItemKind::Atomic
        ) && !item.flags.contains(FragmentItemFlags::HIDDEN)
            && !item.flags.contains(FragmentItemFlags::HANGS)
    }

    /// Item `at` where it is one of the line's own, which every phase but
    /// the annotations' walks.
    #[inline]
    fn line_item(&self, at: FragmentItemId) -> Option<&'a FragmentItem> {
        if self.own.contains(&at) {
            self.items.get(at)
        } else {
            None
        }
    }

    /// The background: the extent of the line's content, where it has any.
    #[inline]
    fn background(&self) -> Option<Background> {
        let mut extent: Option<(InlineLayoutUnit, InlineLayoutUnit)> = None;
        let own = self.items.get_slice(self.own.clone()).unwrap_or_default();
        for item in own.iter().filter(|item| Self::content(item)) {
            work::step();
            let left = item.inline;
            let right = left + item.advance();
            extent = Some(match extent {
                None => (left, right),
                Some((from, to)) => (from.min(left), to.max(right)),
            });
        }
        let (from, to) = extent?;
        Some(Background {
            inline: InlineExtents::from_exact(from, to),
            block: CrossExtents::from_line_height(self.line.metrics().height()),
        })
    }

    /// The next box part to paint, `next`, moving `next` on: the box items in
    /// tree order, each box's left to right, as line layout linked them.
    #[inline]
    fn next_box_part(&self, next: &mut Option<FragmentItemId>) -> Option<Paint<'a>> {
        work::step();
        let at = (*next)?;
        let item = self
            .line_item(at)
            .filter(|item| item.kind() == FragmentItemKind::Box)?;
        *next = item.painted_next();
        Some(Paint::Box(BoxFragment::from_item(self.line, item)))
    }

    /// The next decoration bar of the phase `decorating`, of the block and
    /// then each box in tree order that `decorates` says draws one.
    ///
    /// The line's culled boxes are found in `culled` the first time a phase
    /// looks for one.
    fn next_decoration<F: Fn(NodeKey) -> Decorates>(
        &self,
        decorating: &mut Decorating,
        culled: &mut Option<CulledLine<'a>>,
        decorates: &F,
    ) -> Option<Decoration> {
        let nodes = &self.line.layout().content().nodes;
        loop {
            work::step();
            let node = match decorating.node {
                Some(node) => node,
                None => {
                    // The block: its bars are the whole line's, on its
                    // baseline.
                    decorating.node = Some(NodeId::BLOCK);
                    decorating.at = self.own.start;
                    decorating.end = self.own.end;
                    decorating.baseline = self.line.record().ascent();
                    decorating.next = self.first;
                    continue;
                }
            };
            let asked = decorates(nodes.key(node));
            let draws = if decorating.after_text {
                asked.after_text()
            } else {
                asked.before_text()
            };
            if draws && let Some(bar) = self.bar(decorating, node) {
                return Some(bar);
            }
            // The next box, its parts following one another in the
            // links: its bars are among their items, on their baseline.
            let Some(first) = decorating.next else {
                // Then each culled box that decorates, across the line, on
                // its parent's baseline.
                if !self.has_culled() {
                    return None;
                }
                let (node, from, until) = self.next_culled(decorating, culled, decorates)?;
                decorating.node = Some(node);
                decorating.at = from;
                decorating.end = self.own.end;
                decorating.until = until;
                decorating.baseline = (self.line.layout().fragments())
                    .culled_baseline(nodes, self.line.id(), node, false)
                    .unwrap_or_else(|| self.line.record().ascent());
                continue;
            };
            let head = self.line_item(first)?;
            let mut last = first;
            let mut after = head.painted_next();
            while let Some(at) = after
                && let Some(item) = self.line_item(at)
                && item.node == head.node
            {
                work::step();
                last = at;
                after = item.painted_next();
            }
            decorating.next = after;
            decorating.node = Some(head.node);
            decorating.until = None;
            decorating.at = first;
            decorating.end = self.line_item(last).map_or(last, |item| {
                FragmentItemId::new(last.get() + 1 + item.descendants()).min(self.own.end)
            });
            decorating.baseline = head.block;
        }
    }

    /// Whether the line may hold a culled box that decorates.
    ///
    /// Content with no box that decorates has none to look for, and a line
    /// no inline box holds any of, none of its own.
    #[inline]
    fn has_culled(&self) -> bool {
        let content = self.line.layout().content();
        let boxed = self
            .line
            .head()
            .is_some_and(|head| head.flags.contains(FragmentItemFlags::BOXED));
        content.flags.contains(ContentFlags::DECORATED_BOXES) && (boxed || !work::fast_paths())
    }

    /// The next culled box of the line that draws a decoration of the
    /// phase `decorating`: those open across the line's start first,
    /// outermost first, then those opening on it, in tree order (see the
    /// module documentation).
    fn next_culled(
        &self,
        decorating: &mut Decorating,
        culled: &mut Option<CulledLine<'a>>,
        decorates: &dyn Fn(NodeKey) -> Decorates,
    ) -> Option<(NodeId, FragmentItemId, Option<ClusterId>)> {
        let after_text = decorating.after_text;
        let line = culled.get_or_insert_with(|| self.start_culled(decorates));
        let mut boxes = decorating.culled.unwrap_or(CulledBoxes {
            above: if after_text {
                line.above_after
            } else {
                line.above_before
            },
            item: line.item,
            resume: self.own.start,
        });
        // A box open across the line's start is looked for from it.
        let found = if boxes.above > 0 {
            let node = line
                .drawing
                .around(line.start, boxes.above, decorates, after_text);
            boxes.above -= 1;
            node.map(|node| (node, self.own.start))
        } else {
            self.next_opening(line, &mut boxes, decorates, after_text)
        };
        let in_order = line.in_order;
        let found = found.map(|(node, from)| self.bars_from(in_order, &mut boxes, node, from));
        decorating.culled = Some(boxes);
        found
    }

    /// Returns what both decoration phases read of the line's culled boxes:
    /// the node of the item holding the line's first cluster, and how many
    /// of the boxes around it draw on each side of the text.
    ///
    /// The walk over the kept boxes starts at the line's first box item's
    /// row, where it has one, and otherwise at the line's first node, sought
    /// once.
    fn start_culled(&self, decorates: &dyn Fn(NodeKey) -> Decorates) -> CulledLine<'a> {
        let layout = self.line.layout();
        let (content, analysis) = (layout.content(), layout.analysis());
        let kept = self.line.stages().measured().kept_boxes();
        let record = self.line.record();
        let at = self
            .first
            .and_then(|at| self.line_item(at))
            .and_then(FragmentItem::kept_box)
            .unwrap_or_else(|| {
                let start = content
                    .items
                    .get(record.first_item)
                    .map_or(NodeId::BLOCK, |item| item.node);
                kept.first_from(start)
            });
        let mut drawing = CulledDrawing {
            content,
            kept,
            at,
            variant: self.line.variant(),
        };
        let clusters = record.clusters();
        let in_order = work::fast_paths()
            && !analysis.flags.contains(ParagraphFlags::MIXED_LEVELS)
            && !self.line.level().is_rtl();
        // The item holding the line's first cluster: from the line's first
        // item, past those sitting at its start, the last whose first
        // cluster is not past it.
        let mut first = record.first_item;
        let mut next = ItemId::new(first.get() + 1);
        while content.items.get(next).is_some()
            && analysis.item_clusters.start(next) <= clusters.start
        {
            work::step();
            first = next;
            next = ItemId::new(next.get() + 1);
        }
        let start = content
            .items
            .get(first)
            .map_or(NodeId::BLOCK, |item| item.node);
        // How many around the start draw on each side: one walk up, past
        // the outermost.
        let (above_before, above_after) = drawing.count_around(start, decorates);
        CulledLine {
            drawing,
            start,
            above_before,
            above_after,
            item: ItemId::new(first.get() + 1),
            in_order,
        }
    }

    /// Returns the next culled box opening on the line that draws in the
    /// phase painted after the text where `after_text`, with the item its
    /// bars are looked for from, moving `boxes` past it.
    fn next_opening(
        &self,
        line: &mut CulledLine<'a>,
        boxes: &mut CulledBoxes,
        decorates: &dyn Fn(NodeKey) -> Decorates,
        after_text: bool,
    ) -> Option<(NodeId, FragmentItemId)> {
        let layout = self.line.layout();
        let (content, analysis) = (layout.content(), layout.analysis());
        let clusters = self.line.record().clusters();
        while let Some(item) = content.items.get(boxes.item) {
            work::step();
            if analysis.item_clusters.start(boxes.item) >= clusters.end {
                break;
            }
            boxes.item = ItemId::new(boxes.item.get() + 1);
            let opens = item.kind == ItemKind::Open && !item.flags.contains(ItemFlags::ANNOTATION);
            if opens && line.drawing.draws(item.node, decorates, after_text) {
                return Some((item.node, boxes.resume));
            }
        }
        None
    }

    /// Returns where the bars of culled box `node`, found from `from`, are
    /// looked for: from its first leaf, to the cluster its text ends at.
    ///
    /// On a line `in_order`, the box after it is looked for from that leaf.
    /// Otherwise its bars are looked for over the whole line.
    #[inline]
    fn bars_from(
        &self,
        in_order: bool,
        boxes: &mut CulledBoxes,
        node: NodeId,
        from: FragmentItemId,
    ) -> (NodeId, FragmentItemId, Option<ClusterId>) {
        if !in_order {
            return (node, self.own.start, None);
        }
        let layout = self.line.layout();
        let nodes = &layout.content().nodes;
        let ends = layout.analysis().item_clusters.span(nodes.items(node)).end;
        let first = (from.get()..self.own.end.get())
            .map(FragmentItemId::new)
            .find(|&at| {
                work::step();
                self.items
                    .get(at)
                    .is_some_and(|item| item.is_painted_leaf() && nodes.contains(node, item.node))
            })
            .unwrap_or(self.own.end);
        if boxes.above == 0 && first < self.own.end {
            boxes.resume = first;
        }
        (node, first, Some(ends))
    }

    /// The next bar of `node`'s decoration from where the phase
    /// `decorating` has got to: a run of its text and its descendants' that
    /// nothing else splits.
    #[inline]
    fn bar(&self, decorating: &mut Decorating, node: NodeId) -> Option<Decoration> {
        let layout = self.line.layout();
        let nodes = &layout.content().nodes;
        // The initial letter's box, where `node` is outside it. Chrome sets
        // the box as an atomic inline, which the decorations of the boxes
        // around it do not reach (`AppliedTextDecorations`).
        let letter = self
            .line
            .stages()
            .measured()
            .initial_letter()
            .map(|letter| letter.node)
            .filter(|&letter| !nodes.contains(letter, node));
        let mut run: Option<(InlineLayoutUnit, InlineLayoutUnit)> = None;
        // The first and last items the run decorates.
        let (mut first, mut last) = (decorating.at, decorating.at);
        // Only its parts hold its text, so its bars are found among
        // their items: what lies outside them would start none of its bars
        // and add to none.
        while decorating.at < decorating.end
            && let Some(item) = self.line_item(decorating.at)
        {
            work::step();
            let leaf = item.is_painted_leaf();
            if !leaf {
                decorating.at = FragmentItemId::new(decorating.at.get() + 1);
                continue;
            }
            // A culled box's text ends here, on a line in order: nothing of
            // it is further on.
            if decorating
                .until
                .is_some_and(|until| item.clusters().start >= until)
            {
                decorating.at = decorating.end;
                break;
            }
            let left = item.inline;
            let right = left + item.advance();
            let decorated = item.kind() == FragmentItemKind::Text
                && !item.flags.contains(FragmentItemFlags::HANGS)
                && nodes.contains(node, item.node)
                && letter.is_none_or(|letter| !nodes.contains(letter, item.node));
            match run {
                None if decorated => {
                    run = Some((left, right));
                    first = decorating.at;
                    last = decorating.at;
                }
                None => {}
                // Text of its own right after the run, with no room
                // between: the run goes on.
                Some((from, to)) if decorated && left == to => {
                    run = Some((from, right));
                    last = decorating.at;
                }
                // Something that takes no room splits nothing.
                Some(_) if !decorated && left == right => {}
                // Anything else ends it here, and is looked at again for
                // the next.
                Some(_) => break,
            }
            decorating.at = FragmentItemId::new(decorating.at.get() + 1);
        }
        let (from, to) = run?;
        let items = first..FragmentItemId::new(last.get() + 1);
        Some(self.decoration(node, decorating.baseline, (from, to), items))
    }

    /// The bar of `node`'s decoration from `from` to `to`, on `baseline`,
    /// with its box's font, found over `items`.
    ///
    /// A box's baseline is its items', and the block's the line's, found
    /// when its bars were.
    #[inline]
    fn decoration(
        &self,
        node: NodeId,
        baseline: LayoutUnit,
        (from, to): (InlineLayoutUnit, InlineLayoutUnit),
        items: Range<FragmentItemId>,
    ) -> Decoration {
        let layout = self.line.layout();
        let content = layout.content();
        // The block's first line decorates in its first-line fonts.
        let text = content.nodes.text_facts(node, self.line.variant());
        let used = layout
            .fonts()
            .primary_font(content.facts.text_request(text));
        // On a line `text-fit` scales, the font's metrics scale with the
        // text, as Chrome decorates through the scaled font.
        let scale = self.line.scale();
        let scaled = |px: f32| if scale == 1.0 { px } else { px * scale };
        let (size, ascent, descent) = used.map_or((0.0, 0.0, 0.0), |used| {
            let metrics = &used.metrics;
            (
                scaled(used.size.to_px()),
                scaled(metrics.ascent.to_px()),
                scaled(metrics.descent.to_px()),
            )
        });
        // A line the font does not give has no thickness.
        let line = |offset: LayoutUnit, thickness: LayoutUnit| {
            (thickness > LayoutUnit::ZERO).then(|| FontDecorationLine {
                offset: scaled(offset.to_px()),
                thickness: scaled(thickness.to_px()),
            })
        };
        let underline = used.and_then(|used| {
            line(
                used.metrics.underline_offset,
                used.metrics.underline_thickness,
            )
        });
        let line_through = used.and_then(|used| {
            line(
                used.metrics.strikeout_offset,
                used.metrics.strikeout_thickness,
            )
        });
        Decoration {
            key: content.nodes.key(node),
            inline: InlineExtents::from_exact(from, to),
            baseline: baseline.to_px(),
            size,
            ascent,
            descent,
            underline,
            line_through,
            first: items.start,
            end: items.end,
        }
    }

    /// The next leaf of the text phase from `at`, moving `at` past it: not
    /// the hyphen's items, which it adds to `hyphen` as it passes them, for
    /// the hyphen phase.
    #[inline]
    fn next_leaf(
        &self,
        at: &mut FragmentItemId,
        hyphen: &mut Range<FragmentItemId>,
    ) -> Option<Paint<'a>> {
        while let Some(item) = self.line_item(*at) {
            work::step();
            let here = *at;
            *at = FragmentItemId::new(here.get() + 1);
            if item.flags.contains(FragmentItemFlags::HIDDEN) {
                continue;
            }
            match item.kind() {
                FragmentItemKind::Text => {
                    return Some(Paint::Text(TextRun::new(self.line, item)));
                }
                FragmentItemKind::Atomic => {
                    return Some(Paint::Atomic(Atomic::new(self.line, item)));
                }
                FragmentItemKind::Generated => {
                    let run = TextRun::new(self.line, item);
                    if run.generated() != Some(Generated::Hyphen) {
                        return Some(Paint::Generated(run));
                    }
                    if hyphen.is_empty() {
                        hyphen.start = here;
                    }
                    hyphen.end = *at;
                }
                _ => {}
            }
        }
        None
    }

    /// The next of the hyphen's items, `hyphen`, which the text phase
    /// passed, moving `hyphen` past it.
    #[inline]
    fn next_hyphen(&self, hyphen: &mut Range<FragmentItemId>) -> Option<Paint<'a>> {
        let at = hyphen.start;
        if at >= hyphen.end {
            return None;
        }
        hyphen.start = FragmentItemId::new(at.get() + 1);
        let item = self.line_item(at)?;
        Some(Paint::Generated(TextRun::new(self.line, item)))
    }

    /// The next of the line's annotations' boxes, runs of text or atomic
    /// inlines from `at`, moving `at` past it: each annotation's boxes
    /// first, outermost first, then its runs and atomic inlines.
    #[inline]
    fn next_annotation(&self, at: &mut FragmentItemId) -> Option<Paint<'a>> {
        while self.annotations.contains(at)
            && let Some(item) = self.items.get(*at)
        {
            work::step();
            *at = FragmentItemId::new(at.get() + 1);
            match item.kind() {
                FragmentItemKind::Box => {
                    return Some(Paint::Box(BoxFragment::from_item(self.line, item)));
                }
                FragmentItemKind::Text => {
                    return Some(Paint::Annotation(TextRun::new(self.line, item)));
                }
                FragmentItemKind::Atomic => {
                    return Some(Paint::Atomic(Atomic::new(self.line, item)));
                }
                _ => {}
            }
        }
        None
    }
}

impl<'a, F: Fn(NodeKey) -> Decorates> Iterator for PaintWalk<'a, F> {
    type Item = Paint<'a>;

    fn next(&mut self) -> Option<Paint<'a>> {
        loop {
            // The marks of the run painted last come right after it.
            if let Some(marks) = self.marks.as_mut() {
                if let Some(mark) = marks.next() {
                    return Some(Paint::Emphasis(mark));
                }
                self.marks = None;
            }
            let painted = &self.painted;
            match &mut self.phase {
                Phase::Background => {
                    self.phase = Phase::Boxes(painted.first);
                    if let Some(background) = painted.background() {
                        return Some(Paint::Background(background));
                    }
                }
                Phase::Boxes(next) => match painted.next_box_part(next) {
                    Some(part) => return Some(part),
                    None => self.phase = Phase::BeforeText(Decorating::new(&painted.own, false)),
                },
                Phase::BeforeText(decorating) => {
                    match painted.next_decoration(decorating, &mut self.culled, &self.decorates) {
                        Some(bar) => return Some(Paint::DecorationBeforeText(bar)),
                        None => self.phase = Phase::Annotations(painted.annotations.start),
                    }
                }
                Phase::Annotations(at) => match painted.next_annotation(at) {
                    Some(paint) => {
                        // A run's marks come right after it.
                        if let Paint::Annotation(run) = &paint
                            && run.item().is_marked()
                        {
                            self.marks = Some(EmphasisMarks::new(run));
                        }
                        return Some(paint);
                    }
                    None => {
                        let start = painted.own.start;
                        self.phase = Phase::Text {
                            at: start,
                            hyphen: start..start,
                        };
                    }
                },
                Phase::Text { at, hyphen } => match painted.next_leaf(at, hyphen) {
                    Some(paint) => {
                        if let Paint::Text(run) = &paint
                            && run.item().is_marked()
                        {
                            self.marks = Some(EmphasisMarks::new(run));
                        }
                        return Some(paint);
                    }
                    None => self.phase = Phase::Hyphen(hyphen.clone()),
                },
                Phase::Hyphen(hyphen) => match painted.next_hyphen(hyphen) {
                    Some(paint) => return Some(paint),
                    None => self.phase = Phase::AfterText(Decorating::new(&painted.own, true)),
                },
                Phase::AfterText(decorating) => {
                    match painted.next_decoration(decorating, &mut self.culled, &self.decorates) {
                        Some(bar) => return Some(Paint::DecorationAfterText(bar)),
                        None => self.phase = Phase::Done,
                    }
                }
                Phase::Done => return None,
            }
        }
    }
}
