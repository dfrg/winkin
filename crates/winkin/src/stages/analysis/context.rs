//! What analysis reads from the [`Context`](crate::Context): the word
//! dictionaries, and the host's hyphenator and phrase segmenter.

use alloc::boxed::Box;
use alloc::sync::Arc;

use cleft::Dictionaries;

use crate::segment::{Hyphenator, PhraseSegmenter};

/// What analysis reads besides the content: the dictionaries that break
/// Thai, Lao, Khmer and Myanmar lines, and the host's hyphenator and phrase
/// segmenter.
pub(crate) struct AnalysisContext {
    /// The word dictionaries, shared with the layouts built with them.
    pub(crate) dictionaries: Arc<Dictionaries>,
    /// What `hyphens: auto` text asks for breaks in its words.
    pub(crate) hyphenator: Option<Box<dyn Hyphenator>>,
    /// What `word-break: auto-phrase` text asks for its phrases.
    pub(crate) phrases: Option<Box<dyn PhraseSegmenter>>,
}

impl AnalysisContext {
    /// Returns the context with the dictionaries the crate compiles in, if
    /// any, and no hyphenator or phrase segmenter.
    pub(crate) fn new() -> Self {
        Self {
            dictionaries: Arc::new(compiled_dictionaries()),
            hyphenator: None,
            phrases: None,
        }
    }
}

impl Default for AnalysisContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the dictionaries the `dictionaries` feature compiles in, or none
/// without it.
fn compiled_dictionaries() -> Dictionaries {
    #[cfg(feature = "dictionaries")]
    {
        Dictionaries::with_compiled()
    }
    #[cfg(not(feature = "dictionaries"))]
    {
        Dictionaries::new()
    }
}
