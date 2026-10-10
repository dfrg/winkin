//! Ruby annotations read back: each annotation a line sets over or under
//! its bases, and the text runs on it.
//!
//! **An annotation is a line of its own**: line layout
//! writes each of a line's annotations after the line's own items, an
//! `AnnotationLine` item over its ruby column's box heading the boxes in it
//! that keep a fragment and its text, so a line's
//! [`items`](super::Line::items) are its bases' and never an annotation's,
//! and [`Line::annotations`] reads the rest. Its runs are [`TextRun`]s like
//! any other: glyphs, clusters, fonts and emphasis marks read as a line's
//! own do, placed from the same line box; its boxes are
//! [`BoxFragment`]s, and its atomic inlines [`Atomic`]s.

use super::{Atomic, BoxFragment, Line, TextRun};
use crate::stages::content::NodeKey;
use crate::stages::fragments::{FragmentItem, FragmentItemKind};
use crate::stages::lines::InlineExtents;
use crate::stages::measure::RubySide;
use crate::work;
use core::fmt;
use core::slice::Iter;

impl<'a> Line<'a> {
    /// Returns the text runs of the line's annotations, in order.
    pub(crate) fn annotation_runs(&self) -> AnnotationRuns<'a> {
        AnnotationRuns {
            line: *self,
            items: self.layout().fragments().line_annotations(self.id()).iter(),
        }
    }
}

/// A line's ruby annotations: what [`Line::annotations`] walks.
#[derive(Clone)]
pub(super) struct Annotations<'a> {
    line: Line<'a>,
    /// The annotation lines not handed out yet, each heading its text.
    items: &'a [FragmentItem],
}

impl<'a> Annotations<'a> {
    /// The ruby annotations `line` sets.
    pub(super) fn new(line: Line<'a>) -> Self {
        Self {
            line,
            items: line.layout().fragments().line_annotations(line.id()),
        }
    }
}

impl<'a> Iterator for Annotations<'a> {
    type Item = Annotation<'a>;

    fn next(&mut self) -> Option<Annotation<'a>> {
        loop {
            work::step();
            let (head, rest) = self.items.split_first()?;
            let count = head.descendants().min(rest.len());
            let (text, after) = rest.split_at(count);
            self.items = after;
            if head.kind() == FragmentItemKind::AnnotationLine {
                return Some(Annotation {
                    line: self.line,
                    head,
                    text,
                });
            }
        }
    }
}

/// A ruby annotation element on one line.
///
/// Positioned above or below the column on a separate baseline.
#[derive(Copy, Clone)]
pub struct Annotation<'a> {
    line: Line<'a>,
    head: &'a FragmentItem,
    text: &'a [FragmentItem],
}

impl<'a> Annotation<'a> {
    /// Returns the annotation node key.
    pub fn key(&self) -> NodeKey {
        self.line.layout().content().nodes.key(self.head.node)
    }

    /// Returns column extents along the line, relative to line-box left.
    ///
    /// Uses the maximum width of the base and annotations, including overhang.
    pub fn inline(&self) -> InlineExtents {
        self.head.inline_extents()
    }

    /// Returns the baseline offset from line-box top.
    pub fn baseline(&self) -> f32 {
        self.head.block.to_px()
    }

    /// Returns `true` if the annotation is below the base.
    pub fn is_under(&self) -> bool {
        self.head
            .ruby_level()
            .and_then(|level| self.line.stages().measured().ruby_columns().level(level))
            .is_some_and(|level| level.side == RubySide::Under)
    }

    /// Returns text runs in visual order, from left to right.
    pub fn runs(&self) -> impl Iterator<Item = TextRun<'a>> + Clone + use<'a> {
        AnnotationRuns {
            line: self.line,
            items: self.text.iter(),
        }
    }

    /// Returns atomic inlines in visual order, from left to right.
    pub fn atomics(&self) -> impl Iterator<Item = Atomic<'a>> + Clone + use<'a> {
        let line = self.line;
        self.text
            .iter()
            .filter(|item| item.kind() == FragmentItemKind::Atomic)
            .map(move |item| Atomic::new(line, item))
    }

    /// Returns retained inline-box fragments, outermost first.
    ///
    /// Each fragment is a border box on the annotation line. Its edges occupy
    /// inline space and are painted before text. [`Layout::box_fragments`](crate::Layout::box_fragments)
    /// also returns these boxes and derives culled boxes from their text.
    /// [`descendants`](BoxFragment::descendants) returns zero; use
    /// [`runs`](Self::runs) to access annotation text.
    pub fn boxes(&self) -> impl Iterator<Item = BoxFragment<'a>> + Clone + use<'a> {
        AnnotationBoxes {
            line: self.line,
            items: self.text.iter(),
        }
    }
}

/// The boxes inside one annotation that keep a fragment, outermost first:
/// what [`Annotation::boxes`] walks.
#[derive(Clone)]
struct AnnotationBoxes<'a> {
    line: Line<'a>,
    items: Iter<'a, FragmentItem>,
}

impl<'a> Iterator for AnnotationBoxes<'a> {
    type Item = BoxFragment<'a>;

    fn next(&mut self) -> Option<BoxFragment<'a>> {
        let item = self
            .items
            .by_ref()
            .find(|item| item.kind() == FragmentItemKind::Box)?;
        Some(BoxFragment::from_item(self.line, item))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.items.len()))
    }
}

impl fmt::Debug for Annotation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Annotation")
            .field("key", &self.key())
            .field("inline", &self.inline())
            .field("baseline", &self.baseline())
            .field("under", &self.is_under())
            .finish()
    }
}

/// The text runs of one annotation, in visual order: what
/// [`Annotation::runs`] walks.
#[derive(Clone)]
pub(crate) struct AnnotationRuns<'a> {
    line: Line<'a>,
    items: Iter<'a, FragmentItem>,
}

impl<'a> Iterator for AnnotationRuns<'a> {
    type Item = TextRun<'a>;

    fn next(&mut self) -> Option<TextRun<'a>> {
        let item = self
            .items
            .by_ref()
            .find(|item| item.kind() == FragmentItemKind::Text)?;
        Some(TextRun::new(self.line, item))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.items.len()))
    }
}
