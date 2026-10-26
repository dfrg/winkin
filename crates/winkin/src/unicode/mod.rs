//! Unicode: the character properties and case mapping `cleft`
//! holds, and what layout adds on top of them.
//!
//! The properties are `cleft`'s packed words, read by
//! [`core_props`] for every character and [`rare_props`] per cluster. Its
//! `css` feature gives the classes CSS defines, and its `case` feature the
//! full case mapping `text-transform` reads. Its grapheme segmenter finds
//! the clusters, through [`GraphemeState`] in analysis and [`graphemes`]
//! elsewhere. Its line and word segmenters run in analysis, word motion and
//! `capitalize`.
//!
//! - [`bidi`]: the reordering of resolved levels, and what the bidi resolver
//!   reads of a cluster's classes.
//! - [`normalize`]: what font coverage needs of canonical normalization:
//!   harfrust's pairwise steps and the mark reordering classes.

mod bidi;
mod normalize;

#[cfg(test)]
mod tests;

pub(crate) use bidi::{
    BidiBracket, OVERRIDE_MASK, REMOVED_BY_X9_MASK, cluster_bidi_class, reorder_bidi,
};
pub(crate) use cleft::{
    BidiClass, BracketPairId, CaseLanguage, CoreProps, EastAsianSpacing, EmphasisClass,
    FirstLetterClass, GraphemeClusterBreak, GraphemeState, LineBreak, RareProps, ScriptId,
    TextSpacingClass, core_props, grapheme_props, line_break, lowercase, rare_props, titlecase,
    uppercase,
};
pub(crate) use normalize::{
    MAX_DECOMPOSITION, compose, decompose, is_default_ignorable, is_mark, is_variation_selector,
    modified_combining_class,
};

use core::iter;

use cleft::{Graphemes, Utf8Scalars};

/// Returns the extended grapheme clusters of `text`, each with the byte it
/// starts at.
pub(crate) fn graphemes(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut starts = Graphemes::new(Utf8Scalars::new(text)).peekable();
    iter::from_fn(move || {
        let start = starts.next()?;
        let end = starts.peek().copied().unwrap_or(text.len());
        Some((start, text.get(start..end).unwrap_or_default()))
    })
}
