//! Unicode character properties, and grapheme, word and line segmentation.
//!
//! The crate reads its own compact tables, generated from the Unicode
//! Character Database. It is `no_std`, depends on nothing but `core` and
//! `alloc`, and no lookup or scan allocates or panics, whatever the input.
//!
//! # Properties
//!
//! [`core_props`] and [`rare_props`] return a character's properties, each a
//! single table lookup and a `u32`. [`CoreProps`] holds the ones a text
//! layout reads for every character: the bidi class, the script and its
//! extensions, paired brackets, and the emoji and break-related flags.
//! [`RareProps`] holds the ones read less often: Grapheme_Cluster_Break,
//! East_Asian_Width, Vertical_Orientation and the casing properties.
//! [`line_break`] returns a character's Line_Break.
//!
//! # Segmentation
//!
//! Each segmenter reads a stream of characters with their offsets, such as
//! [`Utf8Scalars`] or [`Utf16Scalars`], and yields the offsets of the
//! boundaries inside it.
//!
//! - [`Graphemes`] finds extended grapheme clusters (UAX #29).
//!   [`GraphemeState`] is its state machine, for a scan that holds the
//!   characters already.
//! - [`LineBreaks`] finds line break opportunities (UAX #14), under the CSS
//!   tailorings [`LineOptions`] selects.
//! - [`WordBreaks`] finds word boundaries (UAX #29), with the options of
//!   [`WordOptions`].
//!
//! Thai, Lao, Khmer and Myanmar have no rules for breaks between words, and
//! Chinese and Japanese words follow no rule either. Inside a run of one of
//! them the line and word segmenters break at the words of a [`Dictionary`],
//! the longest at each position. [`Dictionaries`] holds one per script,
//! compiled in with the `dictionaries` feature or loaded at run time.
//!
//! ```
//! use cleft::{LineBreaks, LineOptions, Utf8Scalars, WordBreaks, WordOptions};
//!
//! let text = "Can't stop, 3.14!";
//! let lines: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), LineOptions::new()).collect();
//! assert_eq!(lines, [6, 12]);
//! let words: Vec<usize> = WordBreaks::new(Utf8Scalars::new(text), WordOptions::new()).collect();
//! assert_eq!(words, [5, 6, 10, 11, 12, 16]);
//! ```
//!
//! The data and rules are of the Unicode version [`UNICODE_VERSION`] names.
//!
//! # Features
//!
//! No feature is on by default.
//!
//! - `css`: the classes CSS defines and Unicode does not, read from the rare
//!   properties: `EastAsianSpacing` for `text-autospace`,
//!   `TextSpacingClass` for `text-spacing-trim`, `EmphasisClass` for
//!   `text-emphasis-skip` and `FirstLetterClass` for `::first-letter`.
//!   Without it a smaller table is compiled, still read by one lookup.
//! - `case`: full case mapping, `uppercase`, `lowercase` and `titlecase`,
//!   with the language tailorings of `CaseLanguage` (about 5 KB of tables).
//! - `dictionaries`: the word dictionaries of Thai, Lao, Khmer, Myanmar, and
//!   Chinese and Japanese, generated from ICU's word lists and compiled in
//!   (about 2.7 MB): `Dictionaries::with_compiled`.
//!
//! # License
//!
//! The crate is licensed under MIT or Apache-2.0. Its tables are generated
//! from the Unicode Character Database, under the Unicode License v3
//! (`LICENSE-UNICODE`). Its dictionaries are generated from ICU's word
//! lists, whose notices are in `THIRD-PARTY`.

#![no_std]
#![deny(missing_docs)]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(feature = "case")]
mod case;
mod dictionary;
mod grapheme;
mod line;
mod props;
mod scalars;
#[cfg(test)]
mod tests;
mod word;

#[cfg(feature = "case")]
pub use case::{CaseLanguage, lowercase, titlecase, uppercase};
pub use dictionary::{Bytes, Dictionaries, Dictionary, DictionaryError, DictionaryScript};
pub use grapheme::{GraphemeProps, GraphemeState, Graphemes, grapheme_props};
pub use line::{LineBreaks, LineOptions, LineStrictness, LineWordOption};
pub use props::{
    BidiClass, BracketPairId, CoreProps, GraphemeClusterBreak, LineBreak, RareProps, ScriptId,
    ScriptWithExt, UNICODE_VERSION, core_props, line_break, rare_props,
};
#[cfg(feature = "css")]
pub use props::{EastAsianSpacing, EmphasisClass, FirstLetterClass, TextSpacingClass};
pub use scalars::{Utf8Scalars, Utf16Scalars};
pub use word::{WordBreaks, WordOptions};

/// The examples of the README, run as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
