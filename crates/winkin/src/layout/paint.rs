//! Paint operations and geometry in Chrome paint order.
//!
//! [`Line::paints`] yields [`Paint`] values. The host draws them using colors,
//! backgrounds, borders and decoration styles retrieved by [`NodeKey`].
//! A line paints its inline content item by item, as CSS 2.1 Appendix E
//! orders it and Chrome paints it:
//!
//! 1. The block background behind the line content.
//! 2. The line's items in visual order, left to right, each box before the
//!    items inside it:
//!    - a retained box fragment: its background and borders;
//!    - a run of text: the underlines and overlines over it, the block's
//!      first and then each box's, outermost first; the text; its emphasis
//!      marks; then its lines through, in the same order;
//!    - an atomic inline;
//!    - a hyphen or an ellipsis.
//! 3. The ruby annotations: each annotation's boxes, then its text and
//!    atomic inlines, with emphasis marks after each run.
//!
//! So a later box's background covers earlier text, its decorations and
//! earlier atomic inlines, and a later run's decorations cover earlier text.
//! On a reordered line the order is visual, not the order of the content.
//! A run's text shadows go under its decorations and its text, and over
//! what is painted before the run.
//!
//! Each decoration bar spans one run of text. Hanging white space has none.
//! An initial letter's text has the decorations of its own box and of the
//! boxes inside it only.
//!
//! Each bar provides reference baseline and font metrics, Chrome-compatible
//! underline helpers, and font underline/strikeout metrics for `from-font`.
//! A box's bars use the baseline of its own fragment, where it keeps one, or
//! else of the innermost retained box around it; block decorations use the
//! line baseline.
//!
//! Only boxes marked by [`ComputedStyle::decorates`](crate::ComputedStyle::decorates)
//! are queried for decorations, retained or culled, and the block always is.
//! Reading paint operations does not allocate.

use core::fmt;
use core::ops::Range;
use core::slice::Iter;

pub use super::emphasis::EmphasisMark;

use super::emphasis::EmphasisMarks;
use super::{BoxFragment, CrossExtents, Line, TextRun};
use crate::data::{Id, Table};
use crate::stages::content::{BoxFlags, ContentFlags, NodeId, NodeKey, NodeKind};
use crate::stages::fragments::{FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind};
use crate::stages::lines::InlineExtents;
use crate::unit::{self, InlineLayoutUnit, LayoutUnit};

use super::boxes::Atomic;
use crate::work;

/// Decoration lines grouped by when they paint.
///
/// Underlines and overlines paint before the text they decorate, and a line
/// through it after. [`Line::paints`] asks the host which lines the block
/// and each box draws.
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
    Background(Background<'a>),
    /// The border box of a retained inline-box fragment.
    ///
    /// Culled boxes are available through
    /// [`Layout::box_fragments`](crate::Layout::box_fragments).
    Box(BoxFragment<'a>),
    /// A decoration line painted before the text: an underline or overline.
    ///
    /// The underline or overline of one decorating box or the block over the
    /// [`Text`](Self::Text) run that follows.
    DecorationBeforeText(Decoration),
    /// A run of a ruby annotation's text, on its annotation's baseline.
    ///
    /// Annotations paint after the rest of the line. See
    /// [`Line::annotations`].
    Annotation(TextRun<'a>),
    /// A run of text.
    Text(TextRun<'a>),
    /// An emphasis mark, after the run whose text it marks.
    ///
    /// See [`TextRun::emphasis_marks`].
    Emphasis(EmphasisMark),
    /// An atomic inline, which the host paints.
    Atomic(Atomic<'a>),
    /// Text the layout made: a hyphen or an ellipsis, where it stands on the
    /// line.
    Generated(TextRun<'a>),
    /// A decoration line painted after the text: a line through it.
    ///
    /// The line through the [`Text`](Self::Text) run before it, of one
    /// decorating box or the block.
    DecorationAfterText(Decoration),
}

/// The block background behind one line of content, as a `::first-line`
/// background is painted.
///
/// Spans the content width, excluding hanging content, and the block's
/// primary font's ascent and descent about the line's baseline, as an inline
/// box's background does. Without a font it spans the line-box height.
#[derive(Copy, Clone)]
pub struct Background<'a> {
    inline: InlineExtents,
    /// Its line, whose font gives the extent across it when asked.
    line: Line<'a>,
}

impl Background<'_> {
    /// Returns inline extents relative to line-box left.
    pub fn inline(&self) -> InlineExtents {
        self.inline
    }

    /// Returns cross-line extents relative to the line-over edge.
    pub fn block(&self) -> CrossExtents {
        // The block's content area, in its first-line font on the first
        // line, as Chrome paints the line's root inline box.
        let line = &self.line;
        let layout = line.layout();
        let content = layout.content();
        let text = content.nodes.text_facts(NodeId::BLOCK, line.variant());
        let request = content.facts.text_request(text);
        let metrics = line.metrics();
        let scale = line.scale();
        let scaled = |px: f32| if scale == 1.0 { px } else { px * scale };
        layout.fonts().primary_font(request).map_or_else(
            || CrossExtents::from_line_height(metrics.height()),
            |used| CrossExtents {
                over: metrics.ascent - scaled(used.metrics.ascent.to_px()),
                under: metrics.ascent + scaled(used.metrics.descent.to_px()),
            },
        )
    }
}

impl PartialEq for Background<'_> {
    /// Compares where it is painted, not which line it was found on.
    fn eq(&self, other: &Self) -> bool {
        self.inline == other.inline && self.block() == other.block()
    }
}

impl fmt::Debug for Background<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Background")
            .field("inline", &self.inline)
            .field("block", &self.block())
            .finish()
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

    /// Returns the computed font size, the basis for decoration thickness
    /// and for lengths in `em`.
    ///
    /// It is the size before `font-size-adjust` and `@font-face`
    /// `size-adjust`, scaled where `text-fit` scales the line.
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
#[derive(Copy, Clone, Debug)]
enum Phase {
    /// The block's background behind the line.
    Background,
    /// The line's own items, from the next to look at.
    Items(FragmentItemId),
    /// The underlines and overlines of text run `run`, past the bar of
    /// `last`: `None` before the first.
    BeforeText {
        run: FragmentItemId,
        last: Option<NodeId>,
    },
    /// The lines through text run `run`, past the bar of `last`.
    AfterText {
        run: FragmentItemId,
        last: Option<NodeId>,
    },
    /// The annotations' items: the next to look at.
    Annotations(FragmentItemId),
    /// Nothing more.
    Done,
}

/// A line's paint, in Chrome's order: what [`Line::paints`] walks.
///
/// It walks the line's items once, left to right, each box part before the
/// items inside it, as Chrome's `BoxFragmentPainter` walks a line's
/// fragment items. A text run's bars are those of the block and of each box
/// around it that decorates, found by walking up from the run's node; that
/// walk runs only in content where a box decorates.
pub(super) struct PaintWalk<'a, F> {
    /// The line and the items it reads.
    painted: PaintedLine<'a>,
    decorates: F,
    /// The emphasis marks of the run painted last, still to hand out.
    marks: Option<EmphasisMarks<'a>>,
    phase: Phase,
    /// The bars made last, each with the node it decorates for: a run's bars
    /// differ from the run before's only in where they lie.
    held: [Option<(NodeId, Decoration)>; 2],
}

/// The line a paint walks, and what its bars read.
struct PaintedLine<'a> {
    line: Line<'a>,
    /// Every item of the layout, of which the walk reads the line's.
    items: &'a Table<FragmentItemId, FragmentItem>,
    /// The line's own items.
    own: Range<FragmentItemId>,
    /// The line's annotation lines, each heading its text.
    annotations: Range<FragmentItemId>,
    /// The lines the block draws.
    block: Decorates,
    /// Whether a box around a run may decorate it: the content has a box
    /// that decorates, and an inline box holds some of the line or is open
    /// across it.
    boxes: bool,
    /// The initial letter's box: the decorations of the boxes around it do
    /// not reach its text.
    letter: Option<NodeId>,
}

impl<'a, F: Fn(NodeKey) -> Decorates> PaintWalk<'a, F> {
    /// What `line` paints, asking `decorates` which decoration lines each key draws.
    pub(super) fn new(line: &Line<'a>, decorates: F) -> Self {
        let layout = line.layout();
        let fragments = layout.fragments();
        let content = layout.content();
        let boxed = line
            .head()
            .is_some_and(|head| head.flags.contains(FragmentItemFlags::BOXED));
        let block = decorates(content.nodes.key(NodeId::BLOCK));
        Self {
            painted: PaintedLine {
                line: *line,
                items: &fragments.items,
                own: fragments.line_item_ids(line.id()),
                annotations: fragments.annotation_ids(line.id()),
                block,
                boxes: content.flags.contains(ContentFlags::DECORATED_BOXES)
                    && (boxed || !work::fast_paths()),
                letter: line
                    .stages()
                    .measured()
                    .initial_letter()
                    .map(|letter| letter.node),
            },
            decorates,
            marks: None,
            phase: Phase::Background,
            held: [None; 2],
        }
    }
}

/// Returns the item after `at`.
#[inline]
fn after(at: FragmentItemId) -> FragmentItemId {
    FragmentItemId::new(at.get() + 1)
}

// The walk is generic, so it is built in the crate that calls it, which
// inlines only what is marked `#[inline]`: each step it takes is. The walk
// up a run's boxes asks `decorates` through a `&dyn Fn` and is built once,
// here.
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

    /// Item `at` where it is one of the line's own.
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
    fn background(&self) -> Option<Background<'a>> {
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
            line: self.line,
        })
    }

    /// The next of the line's items from `at`, moving `at` past it: a paint,
    /// or `Err` with a text run, whose bars go around it.
    #[inline]
    fn next_item(&self, at: &mut FragmentItemId) -> Option<Result<Paint<'a>, FragmentItemId>> {
        while let Some(item) = self.line_item(*at) {
            work::step();
            let here = *at;
            *at = after(here);
            if item.is_leaf() && item.flags.contains(FragmentItemFlags::HIDDEN) {
                continue;
            }
            match item.kind() {
                FragmentItemKind::Box => {
                    return Some(Ok(Paint::Box(BoxFragment::from_item(self.line, item))));
                }
                FragmentItemKind::Text => return Some(Err(here)),
                FragmentItemKind::Atomic => {
                    return Some(Ok(Paint::Atomic(Atomic::new(self.line, item))));
                }
                FragmentItemKind::Generated => {
                    return Some(Ok(Paint::Generated(TextRun::new(self.line, item))));
                }
                FragmentItemKind::Line | FragmentItemKind::AnnotationLine => {}
            }
        }
        None
    }

    /// The next bar over text run `run` past the bar of `last`, painted after
    /// the text where `after_text` and before it otherwise, moving `last` to
    /// it.
    ///
    /// Its bars are the block's, then those of each box around it that
    /// `decorates` says draws one, outermost first. Text that hangs has none,
    /// and an initial letter's text none of the boxes around the letter.
    #[inline]
    fn next_bar<F: Fn(NodeKey) -> Decorates>(
        &self,
        run: FragmentItemId,
        last: &mut Option<NodeId>,
        after_text: bool,
        decorates: &F,
        held: &mut [Option<(NodeId, Decoration)>; 2],
    ) -> Option<Decoration> {
        let item = self.line_item(run)?;
        if item.flags.contains(FragmentItemFlags::HANGS) {
            return None;
        }
        let draws = |sides: Decorates| {
            if after_text {
                sides.after_text()
            } else {
                sides.before_text()
            }
        };
        let block = last.is_none() && draws(self.block);
        let node = if block && self.letter.is_none() {
            NodeId::BLOCK
        } else if self.boxes || self.letter.is_some() {
            let nodes = &self.line.layout().content().nodes;
            self.decorating_box(item.node, *last, block, &|node| {
                draws(decorates(nodes.key(node)))
            })?
        } else {
            return None;
        };
        *last = Some(node);
        Some(self.bar(node, run, item, held))
    }

    /// Returns the outermost box around `node`, or `node` itself, past `last`
    /// that decorates and that `draws`; the block where `block` says it draws
    /// and the run is not an initial letter's.
    ///
    /// The walk up from `node` stops at `last`, and at the initial letter's
    /// box, which the decorations around it do not reach.
    fn decorating_box(
        &self,
        node: NodeId,
        last: Option<NodeId>,
        block: bool,
        draws: &dyn Fn(NodeId) -> bool,
    ) -> Option<NodeId> {
        let content = self.line.layout().content();
        let (nodes, facts) = (&content.nodes, &content.facts);
        let variant = self.line.variant();
        let mut found = None;
        let mut at = node;
        for _ in 0..nodes.len() {
            work::step();
            if at == NodeId::BLOCK {
                return if block { Some(NodeId::BLOCK) } else { found };
            }
            if last.is_some_and(|last| at <= last) {
                break;
            }
            let decorating = self.boxes
                && nodes.kind(at).is_some_and(NodeKind::is_inline_box)
                && facts
                    .box_facts(nodes.box_facts(at, variant))
                    .has(BoxFlags::DECORATES);
            if decorating && draws(at) {
                found = Some(at);
            }
            if self.letter == Some(at) {
                break;
            }
            let parent = nodes.parent(at);
            if parent == at {
                break;
            }
            at = parent;
        }
        found
    }

    /// The bar of `node`'s decoration over text run `run`, `item`, made from
    /// the one `held` keeps for `node` where it keeps one.
    #[inline]
    fn bar(
        &self,
        node: NodeId,
        run: FragmentItemId,
        item: &FragmentItem,
        held: &mut [Option<(NodeId, Decoration)>; 2],
    ) -> Decoration {
        let made = held
            .iter()
            .flatten()
            .find(|(of, _)| *of == node)
            .map(|&(_, bar)| bar);
        let bar = made.unwrap_or_else(|| {
            let bar = self.decoration(node);
            held.rotate_right(1);
            if let Some(slot) = held.first_mut() {
                *slot = Some((node, bar));
            }
            bar
        });
        Decoration {
            inline: InlineExtents::from_exact(item.inline, item.inline + item.advance()),
            first: run,
            end: after(run),
            ..bar
        }
    }

    /// The bar of `node`'s decoration, with its box's font, on its baseline,
    /// over no run yet.
    ///
    /// The block's baseline is the line's. A box's is that of the innermost
    /// box part on the line around it, its own where it keeps one.
    fn decoration(&self, node: NodeId) -> Decoration {
        let layout = self.line.layout();
        let content = layout.content();
        let ascent = self.line.record().ascent();
        let baseline = if node == NodeId::BLOCK {
            ascent
        } else {
            layout
                .fragments()
                .culled_baseline(&content.nodes, self.line.id(), node, false)
                .unwrap_or(ascent)
        };
        // The block's first line decorates in its first-line fonts.
        let text = content.nodes.text_facts(node, self.line.variant());
        let request = content.facts.text_request(text);
        let used = layout.fonts().primary_font(request);
        // On a line `text-fit` scales, the font's metrics scale with the
        // text, as Chrome decorates through the scaled font.
        let scale = self.line.scale();
        let scaled = |px: f32| if scale == 1.0 { px } else { px * scale };
        // Thickness is of the computed size, before `font-size-adjust` and
        // `size-adjust`, as Chrome's `ComputeDecorationThickness` takes
        // `ComputedFontSize`.
        let size = used.map_or(0.0, |_| scaled(content.facts.request(request).font.size));
        let (ascent, descent) = used.map_or((0.0, 0.0), |used| {
            let metrics = &used.metrics;
            (
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
            inline: InlineExtents::default(),
            baseline: baseline.to_px(),
            size,
            ascent,
            descent,
            underline,
            line_through,
            first: self.own.start,
            end: self.own.start,
        }
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
            *at = after(*at);
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
                    self.phase = Phase::Items(painted.own.start);
                    if let Some(background) = painted.background() {
                        return Some(Paint::Background(background));
                    }
                }
                Phase::Items(at) => match painted.next_item(at) {
                    Some(Ok(paint)) => return Some(paint),
                    Some(Err(run)) => self.phase = Phase::BeforeText { run, last: None },
                    None => self.phase = Phase::Annotations(painted.annotations.start),
                },
                Phase::BeforeText { run, last } => {
                    let run = *run;
                    if let Some(bar) =
                        painted.next_bar(run, last, false, &self.decorates, &mut self.held)
                    {
                        return Some(Paint::DecorationBeforeText(bar));
                    }
                    self.phase = Phase::AfterText { run, last: None };
                    if let Some(item) = painted.line_item(run) {
                        let text = TextRun::new(painted.line, item);
                        // A run's marks come right after it.
                        if item.is_marked() {
                            self.marks = Some(EmphasisMarks::new(&text));
                        }
                        return Some(Paint::Text(text));
                    }
                }
                Phase::AfterText { run, last } => {
                    let run = *run;
                    match painted.next_bar(run, last, true, &self.decorates, &mut self.held) {
                        Some(bar) => return Some(Paint::DecorationAfterText(bar)),
                        None => self.phase = Phase::Items(after(run)),
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
                    None => self.phase = Phase::Done,
                },
                Phase::Done => return None,
            }
        }
    }
}
