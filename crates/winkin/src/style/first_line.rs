//! The block's first line: which styles a reader reads in, and the tables kept for it.
//!
//! [`FirstLineVariant`] picks the nodes' own styles or the `::first-line`
//! ones. [`FirstLine`] holds a table for the text and, boxed beside it, the
//! same table for the first line.

use alloc::boxed::Box;
use core::mem::size_of;

use crate::data::HeapBytes;

/// The styles a line is read in: the nodes' own, or their `::first-line` ones.
///
/// This is Blink's `StyleVariant`, in its two kinds.
///
/// Every stage from font selection on keeps a first-line variant of its
/// output in a [`FirstLine`]. The variant covers the first paragraph, the
/// most the first line can reach. Both variants index the same clusters and
/// items, since nothing `::first-line` changes moves a cluster.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum FirstLineVariant {
    /// The nodes' own styles: every line but the block's first, and the
    /// first too where `::first-line` restyles nothing.
    #[default]
    Standard,
    /// The styles `::first-line` gives them: the block's first line.
    FirstLine,
}

/// A table a stage keeps once per first-line variant.
///
/// The font runs, the shaped text and the measured text are such tables. A
/// build clears them. The first line's table stays empty where `::first-line`
/// changes nothing.
pub(crate) trait FirstLineState: Default + HeapBytes {
    /// Empties it, keeping its capacity.
    fn clear(&mut self);

    /// Returns whether it holds nothing, as a first-line variant the build
    /// did not make.
    fn is_empty(&self) -> bool;
}

/// A stage's table for the text, and the same table for the first line.
///
/// The first line's table uses the `::first-line` styles. It is boxed and
/// made the first time a build styles the first line differently, so most
/// layouts hold only `None`. Once made, it is cleared with the text's table
/// and never dropped, so it keeps its capacity. A reader asking for the
/// first line's table gets the text's where the build made none.
pub(crate) struct FirstLine<T> {
    text: T,
    first_line: Option<Box<T>>,
}

impl<T: FirstLineState> FirstLine<T> {
    /// Creates one from the text's table `text`, with no first line's table.
    pub(crate) const fn new(text: T) -> Self {
        Self {
            text,
            first_line: None,
        }
    }

    /// Empties both tables, keeping every allocation.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        if let Some(first_line) = &mut self.first_line {
            first_line.clear();
        }
    }

    /// Returns the text's table, for the stage writing it.
    pub(crate) fn text_mut(&mut self) -> &mut T {
        &mut self.text
    }

    /// Returns the first line's table, where the build made one.
    #[inline]
    pub(crate) fn first_line(&self) -> Option<&T> {
        self.first_line.as_deref().filter(|first| !first.is_empty())
    }

    /// Returns whether the build made the first line's table.
    #[inline]
    pub(crate) fn has_first_line(&self) -> bool {
        self.first_line().is_some()
    }

    /// Returns the table `variant` reads: the first line's where the build
    /// made one, else the text's.
    #[inline]
    pub(crate) fn get(&self, variant: FirstLineVariant) -> &T {
        match variant {
            FirstLineVariant::FirstLine => self.first_line().unwrap_or(&self.text),
            FirstLineVariant::Standard => &self.text,
        }
    }

    /// Returns the first line's table for the stage writing it.
    ///
    /// The first call creates it.
    pub(crate) fn first_line_mut(&mut self) -> &mut T {
        self.first_line.get_or_insert_with(Box::default)
    }

    /// Returns the first line's table if a build made one, empty or not.
    ///
    /// Makes none. A stage uses it to reuse the buffers.
    pub(crate) fn kept_first_line_mut(&mut self) -> Option<&mut T> {
        self.first_line.as_deref_mut()
    }

    /// Returns the heap bytes the first line's table takes, box included,
    /// or zero where no build made one.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.first_line
            .as_deref()
            .map_or(0, |first| size_of::<T>() + first.heap_bytes())
    }
}

impl<T: FirstLineState> HeapBytes for FirstLine<T> {
    fn heap_bytes(&self) -> usize {
        self.text.heap_bytes() + self.first_line_heap_bytes()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /// A stand-in table: some numbers.
    #[derive(Default)]
    struct Numbers(Vec<u32>);

    impl HeapBytes for Numbers {
        fn heap_bytes(&self) -> usize {
            self.0.heap_bytes()
        }
    }

    impl FirstLineState for Numbers {
        fn clear(&mut self) {
            self.0.clear();
        }

        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    /// The first line's table is made on its first write. Reads get it where
    /// it holds something, else the text's table. Clearing keeps it, and its
    /// box and capacity are counted.
    #[test]
    fn the_first_line_is_made_once_and_kept() {
        let mut both = FirstLine::new(Numbers(Vec::new()));
        both.text_mut().0.push(1);
        assert!(!both.has_first_line());
        assert_eq!(both.first_line_heap_bytes(), 0, "nothing made");
        assert_eq!(both.get(FirstLineVariant::FirstLine).0, [1]);
        both.first_line_mut().0.push(2);
        assert_eq!(both.get(FirstLineVariant::FirstLine).0, [2]);
        assert_eq!(both.get(FirstLineVariant::Standard).0, [1]);
        let made = both.first_line_heap_bytes();
        assert!(made >= size_of::<Numbers>() + size_of::<u32>());
        both.clear();
        assert!(!both.has_first_line(), "cleared");
        assert!(both.kept_first_line_mut().is_some(), "kept");
        assert_eq!(both.first_line_heap_bytes(), made, "with its capacity");
        both.first_line_mut().0.push(4);
        assert_eq!(
            both.first_line().map(|first| first.0.as_slice()),
            Some(&[4][..])
        );
    }
}
