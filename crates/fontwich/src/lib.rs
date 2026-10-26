//! Font enumeration, CSS matching and platform fallback.
//!
//! Lists font families without loading metadata, matches fonts to CSS
//! requests, and supports document `@font-face` rules. Fallback uses the
//! platform font APIs on Windows, macOS and Linux, and reads Android's
//! `fonts.xml`. [`Font`]
//! provides bytes for shaping and rendering; this crate performs neither.
//!
//! [`Collection`] searches a stack of [`Layer`]s. [`Family`] provides coverage
//! checks and matching; [`Font`] provides attributes, axes and font data.
//! Fallback uses system layers, or application layers if no system layer
//! exists. Document faces participate in named lookup and matching only.
//! The application traverses CSS family lists and loads pending faces.
//!
//! Family lookup treats `Times`, `Courier` and `Helvetica` as aliases of
//! `Times New Roman`, `Courier New` and `Arial`. Matching follows CSS Fonts 4 with a 14° italic target.
//! [`Font::synthesis`] supplies variation settings, synthetic bold and slant.
//!
//! # System fonts and fallback
//!
//! ```
//! # #[cfg(all(feature = "system", any(windows, unix)))]
//! # fn main() {
//! use fontwich::{
//!     Attributes, Collection, FallbackRequest, FontStyle, FontWeight, GenericClass, Presentation,
//!     Script, parse_language,
//! };
//!
//! // List installed families without loading font metadata.
//! let fonts = Collection::system();
//!
//! // Look up a family, then load its metadata.
//! if let Some(family) = fonts.family("Arial") {
//!     for font in family.fonts() {
//!         let _ = (font.weight(), font.style(), font.charset().contains('A'));
//!     }
//!     // Match bold italic and obtain rendering adjustments.
//!     let bold_italic = Attributes {
//!         style: FontStyle::Italic,
//!         weight: FontWeight::BOLD,
//!         ..Attributes::default()
//!     };
//!     if let Some(font) = family.match_font(bold_italic, true) {
//!         let synthesis = font.synthesis(bold_italic);
//!         let _ = (synthesis.variation_settings(), synthesis.embolden(), synthesis.skew());
//!     }
//! }
//!
//! // Resolve and cache fallback families for Arabic text.
//! let request = FallbackRequest::Text {
//!     script: Script::from_bytes(*b"Arab"),
//!     language: parse_language("ar"),
//!     generic: GenericClass::Plain,
//! };
//! for family in fonts.fallback(&fonts.key(&request)).iter() {
//!     let _ = family.name();
//! }
//! // Search character fallback until a family maps the character.
//! let found = fonts
//!     .char_fallback('﷽', Presentation::Text, &request)
//!     .find(|family| family.covers('﷽'));
//! let _ = found.map(|family| family.name().len());
//! # }
//! # #[cfg(not(all(feature = "system", any(windows, unix))))]
//! # fn main() {}
//! ```
//!
//! # Document fonts
//!
//! Use [`LayerBuilder`] to declare `@font-face` rules and create shared
//! snapshots. Pending faces match their descriptors before bytes are available.
//! [`Font::wants`] identifies characters that require a download.
//!
//! ```
//! use fontwich::{Attributes, Collection, FaceDescriptors, LayerBuilder, Role};
//!
//! // `@font-face { font-family: Brand; src: url(latin.woff2); unicode-range: U+0-FF }`
//! // `@font-face { font-family: Brand; src: url(cyrillic.woff2); unicode-range: U+400-4FF }`
//! let mut document = LayerBuilder::new(Role::Document);
//! let range = |from, to| FaceDescriptors {
//!     unicode_range: std::iter::once(from..=to).collect(),
//!     ..FaceDescriptors::default()
//! };
//! let latin = document.add_face("Brand", range(0x0, 0xFF), None)?;
//! let cyrillic = document.add_face("Brand", range(0x400, 0x4FF), None)?;
//!
//! // Add an application layer beneath the document layer.
//! let installed = LayerBuilder::new(Role::Application);
//! let collection = Collection::new()
//!     .with_layer(installed.snapshot())
//!     .with_layer(document.snapshot());
//!
//! // Find the pending face whose range includes the character.
//! let brand = collection.family("Brand").expect("declared");
//! let fetch = brand
//!     .matching(Attributes::default(), true)
//!     .find(|font| font.wants('ж'))
//!     .and_then(|font| font.face_id());
//! assert_eq!(fetch, Some(cyrillic));
//! assert_ne!(fetch, Some(latin));
//! // Supply downloaded bytes with `document.load_face(cyrillic, bytes, 0)`.
//! // The new generation indicates that layout should be updated.
//! # Ok::<(), fontwich::AddError>(())
//! ```
//!
//! # Character coverage
//!
//! A [`Charset`] contains mapped characters. [`CharsetPage`] supports repeated
//! checks on a single 256-character page.
//!
//! ```
//! use fontwich::Charset;
//!
//! let digits: Charset = ('0'..='9').rev().chain(['5']).collect();
//! assert_eq!(digits.count(), 10);
//! assert!(digits.contains('7'));
//! assert!(!digits.contains('a'));
//!
//! let page = digits.page('5').unwrap();
//! assert!(page.holds('A'));
//! assert!(page.contains('5'));
//! assert!(!page.contains('A'));
//! ```
//!
//! # Loading and caching
//!
//! Family metadata is loaded on first use. Whole files are loaded separately by
//! [`Font::load`]. Fonts from the same file in a layer share loaded bytes while
//! callers retain them. [`Font::key`] identifies the data before loading.
//!
//! [`Collection::key`] canonicalizes fallback requests.
//! [`Collection::fallback`] caches installed families by key.
//! [`Collection::char_fallback`] resolves fallback lists lazily and visits all
//! remaining fallback families if necessary. Name lookup and warm fallback
//! traversal do not allocate.
//!
//! # Features
//!
//! - `system` (default): native font enumeration and fallback. Enables `std`.
//! - `std`: file-backed fonts and directory scanning, without native font APIs.
//! - `mmap`: memory-mapped files for [`Font::load`]. Enables `std`.
//! - `android`: the `fonts.xml` backend, available on any target. Enables
//!   `std`.
//!
//! Without features, the crate uses `no_std` with `alloc` and accepts fonts
//! from byte buffers. Fallback can be supplied through [`FallbackOverride`].
//!
//! # fontconfig loading
//!
//! Linux and BSD builds link libfontconfig by default. Set
//! `RUST_FONTCONFIG_DLOPEN` at build time to load it at runtime instead. This
//! allows building without fontconfig headers or pkg-config and running without
//! libfontconfig installed.
//!
//! The loading mode is an environment setting rather than a Cargo feature, so
//! it does not change other dependencies through feature unification.
//!
//! # Shared types
//!
//! [`Script`], [`Language`], [`GenericFamily`], [`FontStyle`], [`FontWeight`],
//! and [`FontWidth`] are re-exported from [`parlance`].
//!
//! # Read next
//!
//! - [`Collection`], [`Family`] and [`Font`]: the core API.
//! - `docs/architecture.md` in the crate's source: how fontwich is built.

#![no_std]
#![deny(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

extern crate alloc;
// `std` is the feature for files: fonts named by path and read on demand,
// and on Android a resolver that opens them. `system`, the platforms' own
// libraries, brings it in. A build with it off has none of it, which is
// what keeps the feature honest: without this line a stray `use std::` in
// the core stops compiling rather than quietly working, on every host, in
// the `--no-default-features` builds CI runs everywhere.
#[cfg(feature = "std")]
extern crate std;

// The platforms' fallback backends, public.
pub mod backend;
// `LayerBuilder`: layers of fonts the caller supplies, and `@font-face`.
// `Collection`: the stack of layers, lookup, and fallback's entry points.
mod collection;
// Parsing a CSS `font-family` list.
mod css;
// Fallback keys, the answer cache and the walk a missed character makes.
mod fallback;
// The `Family` handle, and family name comparison.
mod family;
// `Font`, its charset and attributes, matching, loading and synthesis.
mod font;
mod hash;
// Heap accounting for `heap_usage`.
mod heap;
// `Layer`: one complete set of family names, loaded lazily.
mod layer;
// Each platform's system layer and fallback backend.
mod platform;
// Reading font files and directories.
#[cfg(feature = "std")]
mod scan;
// The script tags the fallback logic and the platform tables name.
mod script;
mod sort;
// A lock and a one-time cell that block with `std` and spin without it.
mod sync;
#[cfg(test)]
mod test_fonts;

pub use parlance::{
    FontFamilyName, FontStyle, FontWeight, FontWidth, GenericFamily, Language, Script,
};

pub use collection::Collection;
pub use css::parse_font_family;
pub use fallback::{
    FallbackKey, FallbackOverride, FallbackRequest, GenericBucket, GenericClass, Han, Presentation,
    parse_language,
};
pub use family::{Family, names_match};
pub use font::{
    Attributes, Axis, Charset, CharsetPage, FaceDescriptors, FaceId, FaceStyle, Font, FontBytes,
    FontKey, Source, Synthesis,
};
pub use layer::{AddError, Layer, LayerBuilder, LoadFamily, Role};
#[cfg(feature = "std")]
pub use scan::Scanned;

/// The examples of the README, run as doc tests.
#[cfg(all(doctest, feature = "system"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
