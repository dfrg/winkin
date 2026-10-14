//! Static positions for absolutely positioned boxes.
//!
//! Line layout records the position of each anchor. The host uses these
//! positions to lay out and place the boxes.

use core::slice;

use crate::data::Id;
use crate::layout::Layout;
use crate::stages::content::NodeKey;
use crate::stages::fragments::Anchor;
use crate::style::Direction;

/// The static position of an absolutely positioned box.
///
/// Coordinates are in CSS pixels relative to the [`Area`](crate::Area) used
/// for line breaking. In every writing mode, `inline` runs from line-left
/// along the lines, and `block` runs from block-start across them, as in
/// [`FloatPlacement`](crate::FloatPlacement).
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct StaticPosition {
    /// The node key of the box.
    pub key: NodeKey,
    /// The zero-based index of the line containing the anchor.
    ///
    /// `None` after a final forced break, beyond a line clamp, or in content
    /// with no line box. The position then follows a hypothetical empty
    /// line after the last line.
    pub line: Option<usize>,
    /// The inline-start margin edge, measured from line-left of the area.
    ///
    /// An inline-level box follows the anchor after alignment, justification
    /// and bidi reordering. A block-level box uses the inline-start edge of
    /// the area.
    pub inline: f32,
    /// The block-start margin edge, measured from block-start of the area.
    ///
    /// An inline-level box uses the top of the line box. A block-level box
    /// uses the bottom if in-flow content precedes the anchor on the line,
    /// and the top otherwise.
    pub block: f32,
    /// The direction of inline progression.
    ///
    /// With [`Rtl`](Direction::Rtl), `inline` locates the right margin edge.
    /// An inline-level box uses the resolved bidi direction of the anchor;
    /// a block-level box uses the base direction of the block, or of the
    /// current paragraph when the block direction is `auto`.
    pub direction: Direction,
}

impl StaticPosition {
    /// Returns the recorded position with the node key from `layout`.
    fn new(layout: &Layout, anchor: &Anchor) -> Self {
        Self {
            key: layout.content().nodes.key(anchor.node),
            line: anchor.line.map(|line| line.get()),
            inline: anchor.inline.to_px(),
            block: anchor.block.to_px(),
            direction: if anchor.rtl {
                Direction::Rtl
            } else {
                Direction::Ltr
            },
        }
    }
}

/// The static positions line layout recorded, in content order: what
/// [`Layout::static_positions`] walks.
#[derive(Clone)]
pub(super) struct StaticPositions<'a> {
    layout: &'a Layout,
    anchors: slice::Iter<'a, Anchor>,
}

impl<'a> StaticPositions<'a> {
    /// Creates the walk of every static position of `layout`.
    pub(super) fn new(layout: &'a Layout) -> Self {
        Self {
            layout,
            anchors: layout.fragments().anchors().iter(),
        }
    }
}

impl Iterator for StaticPositions<'_> {
    type Item = StaticPosition;

    fn next(&mut self) -> Option<StaticPosition> {
        let anchor = self.anchors.next()?;
        Some(StaticPosition::new(self.layout, anchor))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.anchors.size_hint()
    }
}

impl ExactSizeIterator for StaticPositions<'_> {}
