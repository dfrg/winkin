//! The content's rare tables, boxed and made only where a build has them,
//! and the atomic inline, float and anchor rows they hold.

use super::first_line::FirstLineSource;
use super::map::Map;
use super::{
    AbsoluteId, AtomicId, BreakClearanceId, Content, FloatId, ItemId, KeyedBox, NodeKey,
    TextFitFacts,
};
use crate::build::{BoxSize, Clear, FloatSide, OriginalDisplay};
use crate::data::{Id, Keyed, SortedTable, Table, heap_bytes};
use crate::style::{CalcResolver, ComputedStyle, LengthPercentageOrCalc, Sides, WritingMode};
use crate::unit::LayoutUnit;
use crate::work;

/// An atomic inline's size and margins, keyed by its item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Atomic {
    /// Its item, which holds its U+FFFC.
    pub(crate) item: ItemId,
    /// Its border box and baseline as the host laid it out: finite and not
    /// negative.
    pub(crate) size: BoxSize,
    /// Its style's margins, from which `margin_inline` and `margins_across`
    /// resolve against the content's percentage basis.
    margin: Sides<LengthPercentageOrCalc>,
    /// Its margin box along the line: the border box truncated onto the
    /// grid, as Chrome holds a box's lengths, plus the line-direction
    /// margins on the grid.
    pub(crate) margin_inline: LayoutUnit,
    /// Its margins across the line, `(over, under)`, on the grid. Its
    /// extent reaches this far past its border box.
    pub(crate) margins_across: (LayoutUnit, LayoutUnit),
}

impl Atomic {
    /// Creates the atomic inline of `item`, with `size` as the host laid it
    /// out and margins from `style`, resolved in `writing_mode` against
    /// `basis` pixels, each `calc()` by `calc`.
    pub(super) fn new(
        item: ItemId,
        size: BoxSize,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
        basis: f32,
        calc: Option<&dyn CalcResolver>,
    ) -> Self {
        let mut atomic = Self {
            item,
            size,
            margin: style.edges.margin,
            margin_inline: LayoutUnit::ZERO,
            margins_across: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        };
        atomic.resolve(writing_mode, basis, calc);
        atomic
    }

    /// Recomputes its margin box from its size and margins, resolving
    /// percentages against `basis` pixels and each `calc()` by `calc`.
    pub(super) fn resolve(
        &mut self,
        writing_mode: WritingMode,
        basis: f32,
        calc: Option<&dyn CalcResolver>,
    ) {
        (self.margin_inline, self.margins_across) =
            margin_box(self.size, &self.margin, writing_mode, basis, calc);
    }
}

/// Returns a box's margin box along the line and its margins across the
/// line, `(over, under)`, all on the grid.
///
/// The margin box is the border box `size`, truncated onto the grid as
/// Chrome holds a box's lengths, plus `margin` on either side. Percentages
/// resolve against `basis` pixels and each `calc()` by `calc`, in a block of
/// `writing_mode`.
fn margin_box(
    size: BoxSize,
    margin: &Sides<LengthPercentageOrCalc>,
    writing_mode: WritingMode,
    basis: f32,
    calc: Option<&dyn CalcResolver>,
) -> (LayoutUnit, (LayoutUnit, LayoutUnit)) {
    let margin = margin.resolve(basis, calc);
    let (left, right) = margin.along_line_on_grid(writing_mode);
    let along = LayoutUnit::from_px_truncated(size.inline) + left + right;
    (along, margin.across_line_on_grid(writing_mode))
}

/// How setting a box's size changed it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum SizeChange {
    /// No change.
    None,
    /// Only its block size or baseline changed.
    Across,
    /// Its inline size changed.
    Along,
}

impl Keyed for Atomic {
    type Key = ItemId;

    /// Returns its item, which orders the content's atomic inlines.
    #[inline]
    fn key(&self) -> ItemId {
        self.item
    }
}

/// A search of the atomic inlines for one key, wrapping round from a
/// cursor: from it to the last, then from the first up to it.
///
/// Where keys are unique, it stops at the first match. A search started
/// where the last one ended then costs the distance between them, so
/// looking up keys in reading order costs one pass in total.
pub(crate) struct KeyedAtomics {
    pub(super) key: NodeKey,
    /// Where the search started; it stops on coming back round to it.
    pub(super) start: AtomicId,
    /// The next atomic inline to look at.
    pub(super) next: AtomicId,
    /// Whether the search has wrapped past the last.
    pub(super) wrapped: bool,
    /// Whether no two atomic inlines share a key.
    pub(super) unique: bool,
    /// The last match.
    pub(super) last: Option<AtomicId>,
}

impl KeyedAtomics {
    /// Returns the next atomic inline of `content` with the key, or `None`
    /// once the search has wrapped round (or, with unique keys, after the
    /// first match).
    pub(crate) fn next_match(&mut self, content: &Content) -> Option<AtomicId> {
        if self.unique && self.last.is_some() {
            return None;
        }
        let atomics = content.atomics();
        loop {
            if self.wrapped && self.next >= self.start {
                return None;
            }
            let Some(atomic) = atomics.get(self.next) else {
                if self.wrapped || self.start == AtomicId::new(0) {
                    return None;
                }
                self.wrapped = true;
                self.next = AtomicId::new(0);
                continue;
            };
            work::step();
            let id = self.next;
            self.next = AtomicId::new(id.get() + 1);
            if content.atomic_key(atomic) == self.key {
                self.last = Some(id);
                return Some(id);
            }
        }
    }

    /// Returns where the next search starts: after the last match, or where
    /// this one started if it found none.
    pub(crate) fn cursor(&self) -> AtomicId {
        self.last
            .map_or(self.start, |last| AtomicId::new(last.get() + 1))
    }
}

/// A float's size and side, keyed by its anchor item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Float {
    /// Its anchor: an item with no text, where the float was written.
    pub(crate) item: ItemId,
    /// Its border box as the host laid it out: finite and not negative.
    pub(super) size: BoxSize,
    pub(crate) side: FloatSide,
    /// Its style's margins, from which `margin_box` resolves against the
    /// content's percentage basis.
    margin: Sides<LengthPercentageOrCalc>,
    /// Its margin box, `(along the line, across it)`, never negative.
    ///
    /// It is the border box plus the margins on either side, each truncated
    /// onto the grid as Chrome holds a box's lengths. It is the only size a
    /// float has: the breaker asks the host to place it, and intrinsic
    /// sizes count it. Negative margins clamp the result at zero, as
    /// Chrome's `ClampNegativeToZero` does for a float's contribution.
    pub(crate) margin_box: (LayoutUnit, LayoutUnit),
}

impl Float {
    /// Creates the float anchored at `item` on `side`, with `size` as the
    /// host laid it out and margins from `style`, resolved in
    /// `writing_mode` against `basis` pixels, each `calc()` by `calc`.
    pub(super) fn new(
        item: ItemId,
        size: BoxSize,
        side: FloatSide,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
        basis: f32,
        calc: Option<&dyn CalcResolver>,
    ) -> Self {
        let mut float = Self {
            item,
            size,
            side,
            margin: style.edges.margin,
            margin_box: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        };
        float.resolve(writing_mode, basis, calc);
        float
    }

    /// Recomputes its margin box from its size and margins, resolving
    /// percentages against `basis` pixels and each `calc()` by `calc`.
    pub(super) fn resolve(
        &mut self,
        writing_mode: WritingMode,
        basis: f32,
        calc: Option<&dyn CalcResolver>,
    ) {
        let (along, (over, under)) = margin_box(self.size, &self.margin, writing_mode, basis, calc);
        let across = LayoutUnit::from_px_truncated(self.size.block) + over + under;
        self.margin_box = (along.max(LayoutUnit::ZERO), across.max(LayoutUnit::ZERO));
    }
}

/// An absolutely positioned box's anchor and outer display, keyed by its
/// item.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Absolute {
    /// Its anchor: an item with no text, where the box was written.
    pub(crate) item: ItemId,
    /// Its outer display before positioning blockified it.
    pub(crate) display: OriginalDisplay,
}

impl Keyed for Absolute {
    type Key = ItemId;

    /// Returns its item, which orders the content's anchors.
    #[inline]
    fn key(&self) -> ItemId {
        self.item
    }
}

/// A forced break that clears floats: its item, and the floats it clears.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct BreakClearance {
    /// The break's item, which holds its `\n`.
    pub(crate) item: ItemId,
    pub(crate) clear: Clear,
}

/// What the content keeps only where a build asks for it or has it.
///
/// - The offset map, where the build records one.
/// - The first paragraph as its first line draws it, where that line
///   transforms text differently.
/// - The atomic inlines, floats, absolutely positioned boxes' anchors and
///   clearing breaks.
/// - The keyed boxes and the block's `text-fit`.
///
/// It is boxed and made the first time a build keeps any, so a plain
/// layout holds only a null pointer. Once made, it is cleared with the
/// content and never dropped, so a layout that gains and loses these keeps
/// their capacity.
pub(super) struct ContentExtras {
    pub(super) map: Map,
    pub(super) first_line_source: FirstLineSource,
    /// The atomic inlines, in reading order.
    pub(super) atomics: Table<AtomicId, Atomic>,
    /// The floats, in reading order.
    pub(super) floats: Table<FloatId, Float>,
    /// The absolutely positioned boxes' anchors, in reading order.
    pub(super) absolutes: Table<AbsoluteId, Absolute>,
    /// The forced breaks that clear floats, in reading order.
    pub(super) clearances: Table<BreakClearanceId, BreakClearance>,
    /// The inline boxes, annotations and ruby containers, sorted by key and
    /// then node, for box queries by key.
    ///
    /// The writer pushes each as its node is made and sorts them as it
    /// finishes.
    pub(super) keyed_boxes: SortedTable<KeyedBox>,
    /// The block's `text-fit`.
    pub(super) text_fit: TextFitFacts,
}

impl Default for ContentExtras {
    fn default() -> Self {
        Self::NONE
    }
}

/// The extras that content without a box reads: empty tables.
pub(super) static NO_EXTRAS: ContentExtras = ContentExtras::NONE;

impl ContentExtras {
    /// Empty tables, allocating nothing.
    pub(super) const NONE: Self = Self {
        map: Map::new(),
        first_line_source: FirstLineSource::new(),
        atomics: Table::new(),
        floats: Table::new(),
        absolutes: Table::new(),
        clearances: Table::new(),
        keyed_boxes: SortedTable::new(),
        text_fit: TextFitFacts::NONE,
    };

    /// Returns the offset map if this build records it.
    pub(super) fn recorded_map(&mut self) -> Option<&mut Map> {
        self.map.recording.then_some(&mut self.map)
    }
}

heap_bytes! {
    ContentExtras { map, first_line_source, atomics, floats, absolutes, clearances, keyed_boxes; text_fit }
}
