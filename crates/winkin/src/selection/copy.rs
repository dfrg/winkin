//! The text a selection copies.
//!
//! - The layout's text is what is laid out: white space collapsed, each
//!   element's transform applied, small capitals in the author's case, and
//!   `\n` for each `<br>`. A copy is a slice of it, as `Selection.toString()`
//!   gives it.
//! - The copy leaves out what the builder inserted: an atomic inline's U+FFFC
//!   and a break opportunity's U+200B. Chrome's text iterator emits nothing
//!   for an image or a `<wbr>` either.
//! - A U+200B or soft hyphen in the content stays, as does text an ellipsis
//!   or clamp hides, and ruby annotation text, in source order.
//! - The first line's transform is not in the layout's text, so a copy
//!   leaves it out.
//! - For the clipboard, a no-break space becomes a space, as Blink's plain
//!   text flavour does (`ReplaceNBSPWithSpace`).
//!
//! Chrome's clipboard also copies a node's text before `text-transform`
//! (`IgnoresCssTextTransforms`). The layout does not keep that text; a host
//! that wants it copies its own, located by
//! [`Layout::node_position`](crate::Layout::node_position). The host chooses
//! the clipboard's line ends.
//!
//! Nothing is allocated: the pieces are slices of the layout's text.

use core::fmt;

use super::place::{self, ClusteredPosition};
use super::{Affinity, Position};
use crate::data::{Id, TextOffset};
use crate::layout::Layout;
use crate::stages::analysis::{ClusterClass, ClusterId, Clusters};
use crate::work;

/// How a copy treats no-break spaces.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum CopyKind {
    /// The text as `Selection.toString()` gives it, with no-break spaces kept.
    #[default]
    Text,
    /// Text for the clipboard.
    ///
    /// Converts no-break spaces to spaces.
    Clipboard,
}

/// The text a selection copies, in pieces, for
/// [`Layout::selected_text`](crate::Layout::selected_text).
///
/// The pieces joined, or the `Display` output, form the text.
#[derive(Clone)]
pub(crate) struct SelectedText<'a> {
    text: &'a str,
    clusters: &'a Clusters,
    /// The next cluster to read.
    next: ClusterId,
    /// The selection's end.
    end: TextOffset,
    kind: CopyKind,
}

impl<'a> SelectedText<'a> {
    /// Creates the text a selection from byte `start` to `end` copies under
    /// `kind`.
    pub(crate) fn new(layout: &'a Layout, start: usize, end: usize, kind: CopyKind) -> Self {
        let clusters = &layout.analysis().clusters;
        let start = Position::new(start, Affinity::Downstream);
        let start = place::snap(layout, ClusteredPosition::new(layout, start));
        // The end's cluster is walked to from the start's, costing the
        // selection's length.
        let end = Position::new(end.max(start.offset()), Affinity::Upstream);
        let end = place::snap(
            layout,
            ClusteredPosition::from_walk(layout, start.cluster, end),
        );
        Self {
            text: layout.text(),
            clusters,
            next: start.cluster,
            end: TextOffset::new(end.offset().max(start.offset())),
            kind,
        }
    }

    /// Returns what `cluster` copies as, or `None` where it copies as its own
    /// text.
    ///
    /// Text the builder inserted copies as nothing. A no-break space copies
    /// as a space for the clipboard.
    fn replaced(&self, cluster: ClusterId) -> Option<&'static str> {
        let class = self.clusters.class(cluster)?;
        match class {
            ClusterClass::Object | ClusterClass::BreakOpportunity => Some(""),
            ClusterClass::NoBreakSpace
                if self.kind == CopyKind::Clipboard
                    && self
                        .text
                        .get(self.clusters.range(cluster).start.get()..)?
                        .starts_with('\u{A0}') =>
            {
                Some(" ")
            }
            _ => None,
        }
    }
}

impl<'a> Iterator for SelectedText<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        loop {
            let from = self.clusters.start(self.next);
            if from >= self.end {
                return None;
            }
            // A cluster that does not copy as its own text.
            if let Some(replaced) = self.replaced(self.next) {
                self.next = ClusterId::new(self.next.get() + 1);
                if replaced.is_empty() {
                    continue;
                }
                return Some(replaced);
            }
            // The clusters from here that copy as their own text, in one slice.
            let mut to = self.next;
            loop {
                work::step();
                to = ClusterId::new(to.get() + 1);
                if self.clusters.start(to) >= self.end || self.replaced(to).is_some() {
                    break;
                }
            }
            self.next = to;
            let until = self.clusters.start(to).min(self.end);
            return self.text.get(from.get()..until.get());
        }
    }
}

impl fmt::Display for SelectedText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for piece in self.clone() {
            f.write_str(piece)?;
        }
        Ok(())
    }
}
