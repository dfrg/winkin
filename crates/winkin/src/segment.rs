//! Hyphenation and phrase segmentation supplied by the host.
//!
//! winkin carries no hyphenation dictionaries and no phrase models. A host
//! that wants `hyphens: auto` or `word-break: auto-phrase` sets a
//! [`Hyphenator`] or a [`PhraseSegmenter`] on the [`Context`] with
//! [`Context::set_hyphenator`] or [`Context::set_phrase_segmenter`], with
//! only the languages it ships. Without one, `hyphens: auto` behaves as
//! `manual` and `auto-phrase` as `normal`.
//!
//! A build asks each in text order and keeps what it found in the layout,
//! so a relayout asks neither again. Each call reports its offsets through
//! a callback, so asking allocates nothing in winkin.
//!
//! ```
//! use winkin::segment::Hyphenator;
//! use winkin::style::Language;
//! use winkin::{
//!     Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Layout, NoExclusions,
//!     NodeKey,
//! };
//! # use fontwich::{Collection, LayerBuilder, Role};
//! # let mut fonts = LayerBuilder::new(Role::Application);
//! # fonts.add_data(include_bytes!("../../../support/testing/fonts/Ahem.ttf").as_slice()).unwrap();
//! # let fonts = Collection::new().with_layer(fonts.snapshot());
//!
//! /// Breaks "hyphenation" as "hy-phen-ation", in English only.
//! struct Toy;
//!
//! impl Hyphenator for Toy {
//!     fn hyphenate(&self, word: &str, language: Language, found: &mut dyn FnMut(usize)) {
//!         if language.language() == "en" && word == "hyphenation" {
//!             found(2);
//!             found(6);
//!         }
//!     }
//! }
//!
//! let mut cx = Context::new(fonts);
//! cx.set_hyphenator(Some(Box::new(Toy)));
//!
//! let mut style = ComputedStyle::initial();
//! style.text.language = Language::parse("en").ok();
//! style.text.hyphens = winkin::style::Hyphens::Auto;
//! let block = ComputedBlockStyle::new(&style);
//! let mut layout = Layout::new();
//! let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
//! builder.text(NodeKey(1), "hyphenation");
//! builder.finish();
//!
//! // Ahem draws every character an em, 16 px, wide: "hyphen" and the
//! // hyphen fit in 112 px.
//! layout.break_lines(&mut cx, Area::new(112.0), &mut NoExclusions);
//! assert_eq!(layout.lines().len(), 2);
//! assert!(layout.lines().next().is_some_and(|line| line.is_hyphenated()));
//! ```

use crate::style::Language;
#[cfg(doc)]
use crate::{Context, style::Hyphens, style::WordBreak};

/// Finds where a word may break with a hyphen, for [`Hyphens::Auto`].
///
/// winkin finds the words, asks for each one that is long enough, and
/// applies the limits on what stays before and after a break: CSS's
/// defaults, which Chrome applies, of five characters a word and two on each
/// side. A capitalized English word is not hyphenated, as in Chrome.
///
/// The hyphen drawn at a break is the text's `hyphenate-character`.
pub trait Hyphenator: Send + Sync {
    /// Calls `found` with each byte offset in `word` where it may break with
    /// a hyphen, in increasing order, for text in `language`.
    ///
    /// `word` holds no white space, and neither leading opening punctuation
    /// and quotes nor trailing punctuation. An offset is the length of the
    /// part before the hyphen. Offsets that are 0, at or past the end of
    /// `word`, not on a character boundary, or not greater than the one
    /// before are ignored. Calling `found` for none leaves the word whole,
    /// as for a language without a dictionary.
    fn hyphenate(&self, word: &str, language: Language, found: &mut dyn FnMut(usize));
}

/// Finds the phrases of Japanese text, for [`WordBreak::AutoPhrase`].
///
/// Between two ideographs or kana, a line under `auto-phrase` breaks only
/// at a phrase boundary, as Chrome's phrase segmentation of Japanese does.
/// Elsewhere the usual rules apply. A line that no phrase boundary lets
/// fit breaks as under `normal`.
pub trait PhraseSegmenter: Send + Sync {
    /// Calls `found` with the byte offset in `text` of each phrase after the
    /// first, in increasing order, for text in `language`.
    ///
    /// `text` is the text of one styled run. Offsets that are 0, at or past
    /// the end of `text`, not on a character boundary, or not greater than
    /// the one before are ignored.
    fn segment(&self, text: &str, language: Language, found: &mut dyn FnMut(usize));
}
