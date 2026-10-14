//! Per-item tables: extents, em boxes, and the boxes that keep a fragment.

use crate::data::{Id, Table, find_sorted};
use crate::stages::content::{ItemId, NodeId};
use crate::work;

use super::{EmBoxId, Extent, KeptBoxId};

/// Each item's extent around its baseline (8 bytes), and the strut.
pub(crate) struct ItemExtents {
    pub(super) extents: Table<ItemId, Extent>,
    /// The block's strut, which every line box's extent includes.
    pub(crate) strut: Extent,
}

impl ItemExtents {
    pub(super) const fn new() -> Self {
        Self {
            extents: Table::new(),
            strut: Extent::NONE,
        }
    }

    pub(super) fn clear(&mut self) {
        self.extents.clear();
        self.strut = Extent::NONE;
    }

    /// Reserves room for `count` more items' extents.
    pub(super) fn reserve(&mut self, count: usize) {
        self.extents.reserve(count);
    }

    /// Appends the next item's `extent`.
    pub(super) fn push(&mut self, extent: Extent) {
        self.extents
            .push_bounded(extent, "an extent an item, which an ItemId names");
    }

    /// Replaces the extent of `item`; does nothing past the last item.
    pub(super) fn set(&mut self, item: ItemId, extent: Extent) {
        if let Some(kept) = self.extents.get_mut(item) {
            *kept = extent;
        }
    }

    /// Returns the extent of `item`, or [`Extent::NONE`] past the last.
    #[inline]
    pub(crate) fn get(&self, item: ItemId) -> Extent {
        self.extents.get(item).copied().unwrap_or(Extent::NONE)
    }

    /// Returns how many items have an extent.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.extents.len()
    }
}

/// The em box of each text item, as [`em_box`](super::ruby::em_box) computes
/// it, for placing ruby annotations and emphasis marks without per-line work.
///
/// Filled only where the content has ruby or emphasis marks, and only for
/// items whose fonts share one em box; other items fall back to `em_box`.
/// Each item holds a one-byte [`EmBoxId`] into a table of distinct boxes.
pub(crate) struct ItemEmBoxes {
    pub(super) items: Table<ItemId, EmBoxId>,
    /// The distinct em boxes; the first is [`Extent::NONE`].
    pub(super) boxes: Table<EmBoxId, Extent>,
}

impl ItemEmBoxes {
    pub(super) const fn new() -> Self {
        Self {
            items: Table::new(),
            boxes: Table::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.boxes.clear();
    }

    /// Appends the next item's em box, `em`. Past the [`EmBoxId`] limit the
    /// item gets none.
    pub(super) fn push(&mut self, em: Option<Extent>) {
        let boxes = self.boxes.as_slice();
        let found = em.and_then(|em| boxes.iter().rposition(|&kept| kept == em));
        let id = match (em, found) {
            (_, Some(at)) => EmBoxId::new(at),
            (Some(em), None) => self.boxes.push(em).unwrap_or(EmBoxId::NONE),
            (None, None) => EmBoxId::NONE,
        };
        self.items
            .push_bounded(id, "an em box an item, which an ItemId names");
    }

    /// Pushes [`Extent::NONE`] as the first box, the one
    /// [`EmBoxId::NONE`] names.
    pub(super) fn start(&mut self) {
        if self.boxes.push(Extent::NONE) != Some(EmBoxId::NONE) {
            debug_assert!(false, "a table filled once a build, from empty");
        }
    }

    /// Returns the em box of `item`, or `None` where none is stored.
    #[inline]
    pub(crate) fn get(&self, item: ItemId) -> Option<Extent> {
        let id = *self.items.get(item)?;
        self.boxes.get(id).copied().filter(|em| !em.is_none())
    }
}

/// The inline boxes that keep a fragment, in node order, each with its
/// extent around its baseline. A box with no entry is culled.
pub(crate) struct KeptBoxes {
    pub(super) boxes: Table<KeptBoxId, (NodeId, Extent)>,
}

impl KeptBoxes {
    pub(super) const fn new() -> Self {
        Self {
            boxes: Table::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.boxes.clear();
    }

    /// Returns the extent of `node`'s box fragment, or `None` where the box
    /// is culled or `node` is not an inline box.
    pub(crate) fn get(&self, node: NodeId) -> Option<Extent> {
        find_sorted(self.boxes.as_slice(), node).map(|&(_, extent)| extent)
    }

    /// Returns the extent of kept box `at`, or `None` past the last.
    #[inline]
    pub(crate) fn extent(&self, at: KeptBoxId) -> Option<Extent> {
        self.boxes.get(at).map(|&(_, extent)| extent)
    }

    /// Returns the first kept box at or after `node`, or one past the last.
    /// A walk calls this once, then [`walk_to`](Self::walk_to).
    pub(crate) fn first_from(&self, node: NodeId) -> KeptBoxId {
        work::seek();
        let boxes = self.boxes.as_slice();
        KeptBoxId::new(boxes.partition_point(|&(kept, _)| kept < node))
    }

    /// Returns the first kept box at or after `node`, stepping from `from`
    /// forward or back.
    #[inline]
    pub(crate) fn walk_to(&self, from: KeptBoxId, node: NodeId) -> KeptBoxId {
        let mut at = from;
        while self.node(at).is_some_and(|kept| kept < node) {
            work::step();
            at = KeptBoxId::new(at.get() + 1);
        }
        while let Some(before) = at.get().checked_sub(1).map(KeptBoxId::new)
            && self.node(before).is_some_and(|kept| kept >= node)
        {
            work::step();
            at = before;
        }
        at
    }

    /// Returns the node of kept box `at`, or `None` past the last.
    #[inline]
    pub(crate) fn node(&self, at: KeptBoxId) -> Option<NodeId> {
        self.boxes.get(at).map(|&(node, _)| node)
    }

    /// Returns every kept box's node and extent, in node order.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = (NodeId, Extent)> {
        self.boxes.as_slice().iter().copied()
    }
}
