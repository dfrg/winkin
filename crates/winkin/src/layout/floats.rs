//! Floats read back: which the lines reached, and where the host put each.
//!
//! The host places floats; this reports what it was asked and how it
//! answered. Each record is the float's margin box as the content has it, at
//! the line-left and block-start edges the host gave, in the coordinates the
//! lines were broken in. A host that tracks its own floats knows this
//! already. A host that answers with [`NoExclusions`](crate::NoExclusions)
//! puts each float where its line reaches it and forgets it, so it reads here
//! where to draw each.

use core::slice;

use crate::build::FloatSide;
use crate::layout::Layout;
use crate::stages::content::NodeKey;
use crate::stages::lines::{BlockExtents, InlineExtents, LineFloat};

/// A float placement returned by the host.
///
/// Margin-box coordinates are relative to the [`Area`](crate::Area)
/// used for line breaking.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
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
    /// Its margin box across the block.
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
    /// The floats `records` of those `layout`'s lines placed, in the order
    /// they were placed: one line's, or every one.
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
            // Every record names a float of the content it was broken
            // from; one that did not, which is a bug in the breaker, is
            // passed over.
            if let Some(placement) = self.placement(record) {
                return Some(placement);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.records.len()))
    }
}
