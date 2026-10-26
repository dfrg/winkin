//! Floats read back: which lines reached them, and where the host placed
//! each.
//!
//! Each record is the float's margin box from the content, at the line-left
//! and block-start edges the host returned, in the area's coordinates. A
//! host that tracks its own floats knows this already. A host that answers
//! with [`NoExclusions`](crate::NoExclusions) keeps no record, so it reads
//! here where to draw each float.

use core::slice;

use crate::build::FloatSide;
use crate::layout::Layout;
use crate::stages::content::NodeKey;
use crate::stages::lines::{BlockExtents, InlineExtents, LineFloat};

/// A float and where the host placed it.
///
/// Margin-box coordinates are relative to the [`Area`](crate::Area)
/// used for line breaking.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FloatPlacement {
    /// The node key of the float.
    pub key: NodeKey,
    /// The side it floats to.
    pub side: FloatSide,
    /// The margin-box extents along the line.
    ///
    /// Includes the host-provided position and the size passed to the builder,
    /// including margins. Values are truncated to the 1/64-pixel layout grid.
    pub inline: InlineExtents,
    /// The margin-box extents across the block.
    pub block: BlockExtents,
}

/// Floats as placed, in the order the lines placed them: what
/// [`Line::floats`](crate::Line::floats) and [`Layout::floats`] walk.
#[derive(Clone)]
pub(super) struct Floats<'a> {
    layout: &'a Layout,
    records: slice::Iter<'a, LineFloat>,
}

impl<'a> Floats<'a> {
    /// Creates the walk of `records`, the floats one line or every line of
    /// `layout` placed, in placement order.
    pub(super) fn new(layout: &'a Layout, records: &'a [LineFloat]) -> Self {
        Self {
            layout,
            records: records.iter(),
        }
    }

    fn placement(&self, record: &LineFloat) -> Option<FloatPlacement> {
        let content = self.layout.content();
        let float = content.floats().get(record.float)?;
        let node = content.item_node(float.item);
        let (inline, block) = float.margin_box;
        let (left, top) = (record.left.to_px(), record.top.to_px());
        Some(FloatPlacement {
            key: content.nodes.key(node),
            side: float.side,
            inline: InlineExtents {
                left,
                right: left + inline.to_px(),
            },
            block: BlockExtents {
                start: top,
                end: top + block.to_px(),
            },
        })
    }
}

impl Iterator for Floats<'_> {
    type Item = FloatPlacement;

    fn next(&mut self) -> Option<FloatPlacement> {
        loop {
            let record = self.records.next()?;
            // Every record names a float of the content; one that does not
            // would be a breaker bug, and is skipped.
            if let Some(placement) = self.placement(record) {
                return Some(placement);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.records.len()))
    }
}
