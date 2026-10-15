//! The segmenters analysis caches in the [`Context`](crate::Context), and
//! the host's hyphenator and phrase segmenter.

use alloc::boxed::Box;
use core::array;

use icu_segmenter::{
    GraphemeClusterSegmenter, GraphemeClusterSegmenterBorrowed, LineSegmenterBorrowed,
};

use super::breaks::LineKey;
use crate::segment::{Hyphenator, PhraseSegmenter};
use crate::unicode::line_segmenter;

/// The segmenters analysis runs: a line segmenter for each [`LineKey`], the
/// grapheme segmenter, and the host's hyphenator and phrase segmenter.
///
/// Each ICU segmenter is `Copy` and borrows ICU's baked data, so making one
/// allocates nothing. Each line segmenter carries the Thai, Lao, Khmer and
/// Myanmar breakers: the dictionaries, or the LSTM without the
/// `dictionaries` feature. Text without those scripts never touches them.
pub(crate) struct AnalysisContext {
    lines: [LineSegmenterBorrowed<'static>; LineKey::COUNT],
    graphemes: GraphemeClusterSegmenterBorrowed<'static>,
    /// What `hyphens: auto` text asks for breaks in its words.
    pub(crate) hyphenator: Option<Box<dyn Hyphenator>>,
    /// What `word-break: auto-phrase` text asks for its phrases.
    pub(crate) phrases: Option<Box<dyn PhraseSegmenter>>,
}

impl AnalysisContext {
    /// Makes every segmenter, with no hyphenator or phrase segmenter.
    pub(crate) fn new() -> Self {
        Self {
            lines: array::from_fn(|index| line_segmenter(LineKey::from_index(index).options())),
            graphemes: GraphemeClusterSegmenter::new(),
            hyphenator: None,
            phrases: None,
        }
    }

    /// Returns the line segmenter for `key`.
    pub(super) fn line(&self, key: LineKey) -> LineSegmenterBorrowed<'static> {
        // Every key's index has a slot, since `index` and `COUNT` are
        // defined together; the fallback is never taken.
        match self.lines.get(key.index()) {
            Some(segmenter) => *segmenter,
            None => self.lines[0],
        }
    }

    /// Returns the grapheme segmenter (UAX #29, Unicode 17.0).
    pub(super) fn graphemes(&self) -> GraphemeClusterSegmenterBorrowed<'static> {
        self.graphemes
    }
}

impl Default for AnalysisContext {
    fn default() -> Self {
        Self::new()
    }
}
