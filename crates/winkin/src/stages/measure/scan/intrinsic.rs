//! The intrinsic sizes, added up as the scan goes.
//!
//! Min-content is the widest content between two break opportunities, set
//! as a line of its own, as a breaker in min-content mode sets it.
//! Max-content is the widest paragraph.

use super::LinePay;
use super::ends::AnchoredFloats;
use crate::stages::analysis::ClusterAttrs;
use crate::stages::content::TextFlags;
use crate::stages::measure::tabs::{TabReach, TabStops, tab_advance_reached};
use crate::stages::measure::{IntrinsicWidths, MeasureInput};
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// How far a tab reaches on its paragraph's line, set from the content
/// edge, and on its own min-content line.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Tab {
    paragraph_reach: InlineLayoutUnit,
    min_line_reach: InlineLayoutUnit,
}

impl Tab {
    /// Not a tab: zero either way.
    pub(super) const NONE: Self = Self {
        paragraph_reach: InlineLayoutUnit::ZERO,
        min_line_reach: InlineLayoutUnit::ZERO,
    };

    /// Returns how far it reaches on the paragraph's line.
    pub(super) fn paragraph_reach(self) -> InlineLayoutUnit {
        self.paragraph_reach
    }
}

/// `text-indent` as the intrinsic sizes read it.
///
/// It is resolved in an area of zero width, so a percentage counts as zero:
/// a breaker resolves it so in its min- and max-content modes (Blink's
/// `ComputeTextIndent`). A first line that opens an initial letter takes it
/// twice, as line layout does.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Indent {
    amount: InlineLayoutUnit,
    each_line: bool,
    hanging: bool,
    letter: bool,
}

impl Indent {
    /// Returns the block's indent in `input`. `letter` says the first line
    /// opens an initial letter.
    pub(super) fn new(input: &MeasureInput<'_>, letter: bool) -> Self {
        let indent = input.content.block.text_indent;
        Self {
            amount: InlineLayoutUnit::from_layout(input.indent),
            each_line: indent.each_line,
            hanging: indent.hanging,
            letter,
        }
    }

    /// Returns the indent of a line that is the block's first (`first`), the
    /// first after a forced break (`after_break`), or neither.
    fn line_indent(self, first: bool, after_break: bool) -> InlineLayoutUnit {
        if (first || (self.each_line && after_break)) == self.hanging {
            return InlineLayoutUnit::ZERO;
        }
        if first && self.letter {
            return self.amount + self.amount;
        }
        self.amount
    }
}

/// The intrinsic widths, added up as the scan goes: the widest min-content
/// line and the widest paragraph.
///
/// - **Fitting:** widths are read from the paragraph's start, each end
///   rounded up to the grid. A min-content line ends after the closing
///   edges at its break and before what hangs, taken off by the clusters'
///   own advances.
/// - **Tabs:** a tab takes nothing in the prefix, so it is sized where it
///   lands: on the paragraph's line for max-content, on its own min-content
///   line for min-content. A hanging tab comes off like a hanging space.
/// - **Hanging:** a min-content line leaves out all it ends with that hangs.
///   A paragraph leaves out trailing white space only where all of it
///   hangs: `pre-wrap` space hangs only where it overflows, so max-content
///   counts it (CSS Text 3, "white-space processing"), and `pre` space
///   never hangs. The run's last white space decides for all of it. A
///   hanging mark comes off both sizes under `first`, `last` and
///   `force-end`, and min-content alone under `allow-end` (CSS Text 4,
///   "hanging glyphs").
/// - **Line edges:** a min-content line pays the line-edge costs at both
///   ends; a paragraph at forced breaks and the text's ends.
/// - **Indent:** the block's first paragraph is indented, or every one
///   under `each-line`. A min-content line is indented where it is the
///   block's first, or under `each-line` its paragraph's first. `hanging`
///   inverts both. A tab lands past its line's indent.
#[derive(Default)]
pub(crate) struct IntrinsicScan {
    /// `text-indent`, and which lines it applies to.
    indent: Indent,
    /// The current paragraph is the block's first.
    first_paragraph: bool,
    /// The current min-content line is its paragraph's first.
    first_min_line: bool,
    /// The current paragraph, as its max-content line.
    paragraph: OpenLine,
    /// The current min-content line.
    min_line: OpenLine,
    /// The paragraph's trailing white space hangs only where it overflows, so
    /// max-content counts it.
    tail_conditional: bool,
    /// The margin boxes of the floats anchored in the current paragraph.
    floats: InlineLayoutUnit,
    /// The last cluster offers an opportunity: the min-content line ends at the
    /// next boundary, after its closing edges.
    pending: bool,
    /// The widest min-content lines, and floats among the others.
    min: Widest,
    /// `min` as the current paragraph started, and the paragraph's widest
    /// float: where measuring its lines again starts.
    min_before: Widest,
    paragraph_floats: InlineLayoutUnit,
    /// The current paragraph's min-content lines are measured again whole, so
    /// its end ends none.
    redone: bool,
    /// The widest paragraphs.
    max: Widest,
}

/// A line the intrinsic sizes are adding up: a paragraph's max-content line,
/// or a min-content line.
#[derive(Copy, Clone, Debug, Default)]
struct OpenLine {
    /// Where it starts.
    start: InlineLayoutUnit,
    /// What it pays at its start.
    cost: InlineLayoutUnit,
    /// What the clusters hanging at its end add so far, with its tabs.
    tail: InlineLayoutUnit,
    /// How far its tabs reach on it so far.
    tabs: InlineLayoutUnit,
}

impl OpenLine {
    /// Returns a line starting at `start` that pays `cost` there.
    fn new(start: InlineLayoutUnit, cost: InlineLayoutUnit) -> Self {
        Self {
            start,
            cost,
            ..Self::default()
        }
    }
}

/// The widest lines of one intrinsic size: the block's first, which may be
/// measured in the first line's styles, and the widest other.
#[derive(Copy, Clone, Debug, Default)]
struct Widest {
    first: InlineLayoutUnit,
    other: InlineLayoutUnit,
}

impl Widest {
    /// Takes a line `width` wide, the block's first where `first` says.
    fn take(&mut self, first: bool, width: InlineLayoutUnit) {
        let widest = if first {
            &mut self.first
        } else {
            &mut self.other
        };
        *widest = (*widest).max(width);
    }
}

impl IntrinsicScan {
    /// Returns an empty scan at the block's first paragraph and min-content line.
    pub(super) fn new(indent: Indent) -> Self {
        Self {
            indent,
            first_paragraph: true,
            first_min_line: true,
            ..Self::default()
        }
    }

    /// Returns the indent of the current min-content line.
    fn min_line_indent(&self) -> InlineLayoutUnit {
        self.indent.line_indent(
            self.first_paragraph && self.first_min_line,
            !self.first_paragraph && self.first_min_line,
        )
    }

    /// Returns the indent of the current paragraph's line.
    fn paragraph_indent(&self) -> InlineLayoutUnit {
        self.indent
            .line_indent(self.first_paragraph, !self.first_paragraph)
    }

    /// Returns the offset from a running sum to a position on the current
    /// paragraph's line, past its indent and tabs.
    pub(super) fn paragraph_offset(&self) -> InlineLayoutUnit {
        self.paragraph.tabs - (self.paragraph.start - self.paragraph_indent())
    }

    /// Returns where fitting reads `at` in the current paragraph.
    fn fit(&self, at: InlineLayoutUnit) -> InlineLayoutUnit {
        (at - self.paragraph.start).ceil_to_grid()
    }

    /// Starts the text, whose first lines pay `cost` at their start.
    pub(super) fn start_text(&mut self, cost: LinePay) {
        self.paragraph.cost = cost.edges.start;
        self.min_line.cost = cost.edges.start;
    }

    /// Ends the current min-content line at `at`, paying `end` there.
    fn end_min_line(&mut self, at: InlineLayoutUnit, end: InlineLayoutUnit) {
        let line = self.min_line;
        let width = self.fit(at + line.tabs - line.tail) - self.fit(line.start)
            + line.cost
            + end
            + self.min_line_indent();
        (self.min).take(self.first_paragraph && self.first_min_line, width);
        self.first_min_line = false;
    }

    /// Records a boundary at sum `at`, after its leading closes, where a line
    /// starting or ending pays `cost`.
    ///
    /// Inlined, so a boundary that ends no min-content line costs one test.
    #[inline]
    pub(super) fn boundary(&mut self, at: InlineLayoutUnit, cost: LinePay) {
        if self.pending {
            self.break_min_line(at, cost);
        }
    }

    /// Ends the min-content line owed a break at `at`, and starts the next.
    #[inline(never)]
    fn break_min_line(&mut self, at: InlineLayoutUnit, cost: LinePay) {
        self.end_min_line(at, cost.edges.end - cost.min_end_hang);
        self.min_line = OpenLine::new(at, cost.edges.start);
        self.pending = false;
    }

    /// Replaces a whole ruby column in the min-content lines by its pieces.
    ///
    /// The first and last pieces stay glued to the text beside them; the
    /// widest is a min-content line of its own.
    pub(super) fn ruby(
        &mut self,
        at: InlineLayoutUnit,
        whole: LayoutUnit,
        first: LayoutUnit,
        last: LayoutUnit,
        widest: LayoutUnit,
    ) {
        let first_end = at - InlineLayoutUnit::from_layout(whole - first);
        self.min_line.tail = InlineLayoutUnit::ZERO;
        self.end_min_line(first_end, InlineLayoutUnit::ZERO);
        self.min.other = self.min.other.max(InlineLayoutUnit::from_layout(widest));
        let start = at - InlineLayoutUnit::from_layout(last);
        self.min_line = OpenLine::new(start, InlineLayoutUnit::ZERO);
    }

    /// Returns how far a tab at `pen` reaches on the paragraph's line and on
    /// the min-content line.
    ///
    /// Each line measures from its start, past its indent and earlier tabs. A
    /// ruby column's overhang may reach into it by `reach`.
    pub(super) fn tab(
        &self,
        pen: InlineLayoutUnit,
        stops: TabStops,
        reach: Option<TabReach>,
    ) -> Tab {
        let reach = |line: OpenLine, indent: InlineLayoutUnit| {
            let position = pen - (line.start - indent) + line.tabs;
            let width = tab_advance_reached(LayoutUnit::ZERO, position, stops, reach);
            InlineLayoutUnit::from_layout(width)
        };
        Tab {
            paragraph_reach: reach(self.paragraph, self.paragraph_indent()),
            min_line_reach: reach(self.min_line, self.min_line_indent()),
        }
    }

    /// Records a cluster with `attrs` that adds `own` to the prefix and, as a
    /// tab, `tab` to its lines. `text` is its text facts' flags.
    pub(super) fn cluster(
        &mut self,
        attrs: Option<ClusterAttrs>,
        own: InlineLayoutUnit,
        tab: Tab,
        text: TextFlags,
    ) {
        let has = |bit| attrs.is_some_and(|attrs| attrs.has(bit));
        self.paragraph.tabs += tab.paragraph_reach;
        self.min_line.tabs += tab.min_line_reach;
        if has(ClusterAttrs::HANGS) {
            self.min_line.tail += own + tab.min_line_reach;
            self.paragraph.tail += own + tab.paragraph_reach;
            // A separator or generated opportunity takes no room, so decides nothing.
            if attrs.is_some_and(|attrs| attrs.class().is_breaking_space()) {
                self.tail_conditional = text.contains(TextFlags::HANGS_CONDITIONALLY);
            }
        } else {
            self.min_line.tail = InlineLayoutUnit::ZERO;
            self.paragraph.tail = InlineLayoutUnit::ZERO;
            self.tail_conditional = false;
        }
        self.pending = has(ClusterAttrs::BREAK_AFTER)
            || (text.contains(TextFlags::ANYWHERE) && has(ClusterAttrs::EMERGENCY_AFTER));
    }

    /// Records the floats anchored at a boundary of the current paragraph.
    ///
    /// A max-content line sets them all beside it, and no line is narrower
    /// than the widest, in both min- and max-content modes. `clear` is the
    /// host's concern, so every float sits beside the paragraph.
    pub(super) fn floats(&mut self, anchored: AnchoredFloats) {
        self.floats += anchored.sum;
        self.min.other = self.min.other.max(anchored.widest);
        self.paragraph_floats = self.paragraph_floats.max(anchored.widest);
    }

    /// Ends the paragraph at `at`, after its closing edges, and starts the
    /// next. A line either side pays `cost`.
    pub(super) fn end_paragraph(&mut self, at: InlineLayoutUnit, cost: LinePay) {
        if !self.redone {
            self.end_min_line(at, cost.edges.end - cost.min_end_hang);
        }
        let line = self.paragraph;
        let hung = if self.tail_conditional {
            InlineLayoutUnit::ZERO
        } else {
            line.tail
        };
        let width = self.fit(at + line.tabs - hung)
            + line.cost
            + cost.edges.end
            + self.paragraph_indent()
            + self.floats;
        self.max.take(self.first_paragraph, width);
        self.floats = InlineLayoutUnit::ZERO;
        self.first_paragraph = false;
        self.first_min_line = true;
        self.paragraph = OpenLine::new(at, cost.edges.start);
        self.min_line = OpenLine::new(at, cost.edges.start);
        self.tail_conditional = false;
        self.pending = false;
        self.min_before = self.min;
        self.paragraph_floats = InlineLayoutUnit::ZERO;
        self.redone = false;
    }

    /// Restarts the current paragraph's min-content lines, from the widest
    /// before it and its floats, to measure them again whole.
    pub(super) fn restart_min_lines(&mut self) {
        self.min = self.min_before;
        self.min.other = self.min.other.max(self.paragraph_floats);
        self.redone = true;
    }

    /// Returns the width of a min-content line from `start` to `end`, where its
    /// content ends; its paragraph's first where `first`.
    pub(super) fn min_line_width(
        &self,
        start: InlineLayoutUnit,
        end: InlineLayoutUnit,
        first: bool,
    ) -> InlineLayoutUnit {
        let indent = self.indent.line_indent(
            self.first_paragraph && first,
            !self.first_paragraph && first,
        );
        self.fit(end) - self.fit(start) + indent
    }

    /// Takes a min-content line `width` wide, its paragraph's first where
    /// `first`.
    pub(super) fn take_min_line(&mut self, width: InlineLayoutUnit, first: bool) {
        (self.min).take(self.first_paragraph && first, width);
    }

    /// Returns the sizes, taking the block's first-line widths from `first`
    /// where the first line has a variant of its own.
    ///
    /// Neither size is negative, and max-content is never under min-content.
    /// The block's first min-content line and first paragraph are measured in
    /// the first line's styles.
    pub(crate) fn finish(&self, first: Option<&IntrinsicScan>) -> IntrinsicWidths {
        let first = first.unwrap_or(self);
        let min = (first.min.first.max(self.min.other))
            .to_layout()
            .max(LayoutUnit::ZERO);
        IntrinsicWidths {
            min,
            max: first.max.first.max(self.max.other).to_layout().max(min),
        }
    }
}
