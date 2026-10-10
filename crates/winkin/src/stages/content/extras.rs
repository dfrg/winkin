//! What the content keeps only where a build has it: the atomic inlines,
//! the floats, the absolutely positioned boxes' anchors, the clearing
//! breaks, the offset map and the first line's side text.

use super::first_line::FirstLineSource;
use super::map::Map;
use super::{AbsoluteId, AtomicId, BreakClearanceId, Content, FloatId, ItemId, KeyedBox, NodeKey};
use crate::build::{BoxSize, Clear, FloatSide, OriginalDisplay};
use crate::data::{Id, Keyed, SortedTable, Table, heap_bytes};
use crate::style::{ComputedStyle, LengthPercentage, Sides, WritingMode};
use crate::unit::LayoutUnit;
use crate::work;

/// An atomic inline's size, found from its item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Atomic {
    /// Its item, which holds its U+FFFC.
    pub(crate) item: ItemId,
    /// Its border box and baseline as the host laid it out: finite and not
    /// negative.
    pub(crate) size: BoxSize,
    /// Its style's margins, which `margin_inline` and `margins_across` are
    /// resolved from against the content's percentage basis.
    margin: Sides<LengthPercentage>,
    /// Its margin box along the line. This is its border box, truncated
    /// onto the grid as Chrome holds a box's lengths, plus its style's
    /// margins along the line on either side, on the grid.
    pub(crate) margin_inline: LayoutUnit,
    /// Its style's margins across the line, `(over, under)`, on the grid,
    /// which its extent reaches past its border box.
    pub(crate) margins_across: (LayoutUnit, LayoutUnit),
}

impl Atomic {
    /// Makes the atomic inline of `item`, `size` as the host laid it out,
    /// set in `style` in a block of `writing_mode` whose percentages are of
    /// `basis` pixels. The writer lowers its margin box here.
    pub(super) fn new(
        item: ItemId,
        size: BoxSize,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
        basis: f32,
    ) -> Self {
        let mut atomic = Self {
            item,
            size,
            margin: style.edges.margin,
            margin_inline: LayoutUnit::ZERO,
            margins_across: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        };
        atomic.resolve(writing_mode, basis);
        atomic
    }

    /// Lowers its margin box again from its size and margins, their
    /// percentages taken of `basis` pixels.
    pub(super) fn resolve(&mut self, writing_mode: WritingMode, basis: f32) {
        (self.margin_inline, self.margins_across) =
            margin_box(self.size, &self.margin, writing_mode, basis);
    }
}

/// Returns a box's margin box along the line, and its margins across the
/// line, `(over, under)`, all on the grid.
///
/// The margin box is the border box `size`, truncated onto the grid as
/// Chrome holds a box's lengths, plus `margin` on either side. The margins'
/// percentages are taken of `basis` pixels, in a block of `writing_mode`.
fn margin_box(
    size: BoxSize,
    margin: &Sides<LengthPercentage>,
    writing_mode: WritingMode,
    basis: f32,
) -> (LayoutUnit, (LayoutUnit, LayoutUnit)) {
    let margin = margin.resolve(basis);
    let (left, right) = margin.along_line_on_grid(writing_mode);
    let along = LayoutUnit::from_px_truncated(size.inline) + left + right;
    (along, margin.across_line_on_grid(writing_mode))
}

/// How setting a box's size changed it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum SizeChange {
    /// It kept its size.
    None,
    /// Only its block size or baseline changed.
    Across,
    /// Its inline size changed.
    Along,
}

impl Keyed for Atomic {
    type Key = ItemId;

    /// Its item: the content's atomic inlines are in item order.
    #[inline]
    fn key(&self) -> ItemId {
        self.item
    }
}

/// A search of the atomic inlines for those with one key, round from a
/// cursor: from it to the last, then from the first up to it.
///
/// Where no two atomic inlines share a key, it ends at the first it finds.
/// A search started where the last ended then costs the distance between
/// the two, so keys in reading order cost one pass in total.
pub(crate) struct KeyedAtomics {
    pub(super) key: NodeKey,
    /// Where the search started, and stops on coming round to it.
    pub(super) start: AtomicId,
    /// The next atomic inline to look at.
    pub(super) next: AtomicId,
    /// Whether the search has come round past the last.
    pub(super) wrapped: bool,
    /// Whether no two atomic inlines share a key.
    pub(super) unique: bool,
    /// The last atomic inline found.
    pub(super) last: Option<AtomicId>,
}

impl KeyedAtomics {
    /// Returns the next atomic inline of `content` with the key, or `None`
    /// once the search has come round.
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

    /// Returns where the next search starts: after the last atomic inline
    /// found, or where this one started if it found none.
    pub(crate) fn cursor(&self) -> AtomicId {
        self.last
            .map_or(self.start, |last| AtomicId::new(last.get() + 1))
    }
}

/// A float's size and side, found from its anchor item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Float {
    /// Its anchor: an item with no text, where the float was written.
    pub(crate) item: ItemId,
    /// Its border box as the host laid it out: finite and not negative.
    pub(super) size: BoxSize,
    /// The side it floats to.
    pub(crate) side: FloatSide,
    /// Its style's margins, which `margin_box` is resolved from against
    /// the content's percentage basis.
    margin: Sides<LengthPercentage>,
    /// Its margin box, as `(along the line, across it)`, never negative.
    ///
    /// It is the border box as the host laid it out plus its style's
    /// margins on either side. Each is truncated onto layout's grid, as
    /// Chrome holds a box's lengths. The writer lowers it.
    ///
    /// This is the one size a float takes. The breaker asks the host to
    /// place it, and the intrinsic sizes count it. Negative margins that
    /// leave less than nothing take nothing, as Chrome clamps a float's
    /// contribution (`ClampNegativeToZero`).
    pub(crate) margin_box: (LayoutUnit, LayoutUnit),
}

impl Float {
    /// Makes the float anchored at `item`, `size` as the host laid it out, on
    /// `side`, set in `style` in a block of `writing_mode` whose percentages
    /// are of `basis` pixels.
    pub(super) fn new(
        item: ItemId,
        size: BoxSize,
        side: FloatSide,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
        basis: f32,
    ) -> Self {
        let mut float = Self {
            item,
            size,
            side,
            margin: style.edges.margin,
            margin_box: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        };
        float.resolve(writing_mode, basis);
        float
    }

    /// Lowers its margin box again from its size and margins, their
    /// percentages taken of `basis` pixels.
    pub(super) fn resolve(&mut self, writing_mode: WritingMode, basis: f32) {
        let (along, (over, under)) = margin_box(self.size, &self.margin, writing_mode, basis);
        let across = LayoutUnit::from_px_truncated(self.size.block) + over + under;
        self.margin_box = (along.max(LayoutUnit::ZERO), across.max(LayoutUnit::ZERO));
    }
}

/// An absolutely positioned box's anchor and its outer display, found from
/// its item.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Absolute {
    /// Its anchor: an item with no text, where the box was written.
    pub(crate) item: ItemId,
    /// Its outer display before positioning blockified it.
    pub(crate) display: OriginalDisplay,
}

impl Keyed for Absolute {
    type Key = ItemId;

    /// Its item: the content's anchors are in item order.
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
///   transforms some text its own way.
/// - The atomic inlines, the floats and the absolutely positioned boxes.
///
/// It is boxed, and made the first time a build keeps any, so a plain
/// layout holds only the box's pointer. Once made, it is cleared with the
/// content and never dropped. A layout that gains and loses them keeps
/// their capacity.
#[derive(Default)]
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
    /// then node, which a box query by key reads.
    ///
    /// The writer pushes each as its node is made, and sorts them as it
    /// finishes.
    pub(super) keyed_boxes: SortedTable<KeyedBox>,
}

/// What content that keeps no extras reads them as: none of each.
pub(super) static NO_EXTRAS: ContentExtras = ContentExtras::NONE;

impl ContentExtras {
    /// None of each, allocating nothing.
    pub(super) const NONE: Self = Self {
        map: Map::new(),
        first_line_source: FirstLineSource::new(),
        atomics: Table::new(),
        floats: Table::new(),
        absolutes: Table::new(),
        clearances: Table::new(),
        keyed_boxes: SortedTable::new(),
    };

    /// The offset map, where this build records it.
    pub(super) fn recorded_map(&mut self) -> Option<&mut Map> {
        self.map.recording.then_some(&mut self.map)
    }
}

heap_bytes! {
    ContentExtras { map, first_line_source, atomics, floats, absolutes, clearances, keyed_boxes }
}
