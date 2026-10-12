//! CSS inline layout for one block.
//!
//! Lays out styled text, inline boxes, atomic inlines, floats, forced breaks
//! and ruby. Provides positioned glyphs and boxes, paint order, carets, hit
//! testing, selection geometry and static positions for absolute positioning.
//! [`config::Config`] holds the choices CSS leaves open or where
//! implementations differ.
//!
//! The host supplies computed styles, font loading, float placement and
//! painting. winkin handles whitespace, bidi, font selection and fallback,
//! shaping, line breaking, alignment, justification and vertical writing.
//!
//! # Quick start
//!
//! Build content in document order with [`LayoutBuilder`]. Finishing prepares
//! width-independent data. Call [`Layout::break_lines`] for each layout width;
//! subsequent calls reuse that data. Read results through lines, items and glyphs:
//!
//! ```
//! use fontwich::{Collection, LayerBuilder, Role};
//! use winkin::{
//!     Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Item, Layout, NoExclusions,
//!     NodeKey,
//! };
//! # // Ahem, every glyph of which is an em square.
//! # let font_bytes = include_bytes!("../../../support/testing/fonts/Ahem.ttf").as_slice();
//!
//! // Add a font from bytes. With the fontwich `system` feature,
//! // `Collection::system()` provides installed fonts.
//! let mut fonts = LayerBuilder::new(Role::Application);
//! fonts.add_data(font_bytes)?;
//! let mut cx = Context::new(Collection::new().with_layer(fonts.snapshot()));
//!
//! // A plain string: one text node under the block's own style.
//! let mut layout = Layout::new();
//! let style = ComputedStyle::initial();
//! let block = ComputedBlockStyle::new(&style);
//! let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
//! builder.text(NodeKey(1), "Hello, world");
//! assert!(builder.finish().is_complete());
//!
//! // Relayout at another width without repeating preparation.
//! layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
//! assert_eq!(layout.lines().len(), 1);
//! layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
//! assert_eq!(layout.lines().len(), 2);
//!
//! // Read items in visual order. Glyph positions are relative to line-box
//! // left and top. Reading the layout requires no context.
//! let mut glyphs = 0;
//! for line in layout.lines() {
//!     let metrics = line.metrics();
//!     for item in line.items() {
//!         if let Item::Text(run) = item {
//!             // `None` where no font draws the run, which then draws nothing.
//!             let Some(font) = run.font() else { continue };
//!             assert!(!font.data().is_empty() && font.size == 16.0);
//!             for glyph in run.glyphs() {
//!                 let (x, y) = (metrics.left + glyph.x, metrics.top + glyph.y);
//!                 assert!(x >= 0.0 && y > metrics.top);
//!                 glyphs += 1;
//!             }
//!         }
//!     }
//! }
//! // "Hello," and "world": the trailing space at the break is removed, as in
//! // Chrome.
//! assert_eq!(glyphs, 11);
//! # Ok::<(), fontwich::AddError>(())
//! ```
//!
//! # Building
//!
//! Supply computed styles through [`ComputedStyle`] and [`ComputedBlockStyle`].
//! The builder supports:
//! - [`text`](LayoutBuilder::text), and [`open_box`](LayoutBuilder::open_box) and
//!   [`close_box`](LayoutBuilder::close_box) for inline boxes;
//! - [`atomic`](LayoutBuilder::atomic) for inline-blocks and replaced
//!   elements, and [`float`](LayoutBuilder::float) for floats, each laid out
//!   by the host first;
//! - [`absolute`](LayoutBuilder::absolute) for absolutely positioned boxes,
//!   which take no room;
//! - [`line_break`](LayoutBuilder::line_break),
//!   [`line_break_clearing`](LayoutBuilder::line_break_clearing) and
//!   [`break_opportunity`](LayoutBuilder::break_opportunity) for `<br>`,
//!   `<br clear>` and `<wbr>`;
//! - [`open_ruby`](LayoutBuilder::open_ruby),
//!   [`open_annotation`](LayoutBuilder::open_annotation) and
//!   [`open_annotation_with_position`](LayoutBuilder::open_annotation_with_position)
//!   for ruby;
//! - [`set_first_letter`](LayoutBuilder::set_first_letter) for
//!   `::first-letter` and `initial-letter`.
//!
//! Builds report dropped or replaced content through [`BuildReport`] rather
//! than failing when layout limits are exceeded.
//!
//! # Reading
//!
//! - [`Layout::lines`] and [`Line::items`]: the lines and their text runs,
//!   boxes, atomic inlines and generated text, in visual order.
//! - [`Line::paints`]: paint operations in Chrome order ([`paint`]).
//! - [`Line::annotations`]: the ruby annotations over and under a line.
//! - [`Layout::metrics`], [`Layout::intrinsic_sizes`] and
//!   [`Layout::room_below`]: measurements for host block layout.
//! - [`Layout::box_fragments`]: where an inline box landed, for client rects.
//! - [`Layout::floats`] and [`Layout::static_positions`]: float placements
//!   and static positions for absolutely positioned boxes.
//! - [`selection`]: carets, hit tests, motion, selection rectangles and copy.
//! - [`path`]: lines set along curves, as SVG's `textPath` sets them.
//!
//! Floats and other exclusions reach the breaker through the [`Exclusions`]
//! trait, which the host implements for its block formatting context;
//! [`NoExclusions`] serves a block with none.
//!
//! # Fonts
//!
//! A [`Context`] holds a [`fontwich::Collection`], and selects fonts from it
//! by CSS font matching and the platform's fallback. Web
//! fonts still downloading are listed by [`Layout::wanted_faces`]. A host
//! that draws with hinted metrics supplies them through
//! [`font::FontMetricsProvider`]. Contexts created with one [`SharedFonts`]
//! parse each font once between them.
//!
//! # Features
//!
//! - `std` (default): passes `std` on to fontwich, read-fonts and harfrust,
//!   and provides [`SharedFonts`]. Without it the crate is `no_std` with
//!   `alloc`.
//! - `dictionaries` (default): ICU's dictionaries segment Thai, Lao, Khmer
//!   and Myanmar lines, and Chinese and Japanese words.
//!   Without it, an LSTM model segments the Southeast Asian scripts and
//!   Unicode's word rules find Chinese and Japanese words; the binary is
//!   about 3.5 MB smaller.
//!
//! # Read next
//!
//! - `docs/css-support.md` in the crate's source: every CSS property winkin
//!   reads, how far each goes, the [`config::Config`] fields, and what the
//!   host does.
//! - [`LayoutBuilder`] and [`Layout`]: the core API.
//! - The `advance_cache` and `paragraph` examples.

#![no_std]
#![warn(missing_docs)]
// A path in code names an imported item or its module, never a full path.
// `clippy.toml` sets how many segments are too many.
#![warn(clippy::absolute_paths)]

extern crate alloc;
// The tests time themselves, which needs a clock, with or without the
// feature.
#[cfg(any(feature = "std", test))]
extern crate std;

// The public API ------------------------------------------------------------

// The builder a caller pushes content through.
mod build;
pub mod config;
pub mod font;
// The fonts, caches and scratch that every layout is built and broken with.
mod context;
// A layout, the two orchestrators that prepare and break it, and the views it
// is read back through: lines, items, text runs, glyphs, boxes and the block.
// What a line paints is the public module `paint`, re-exported below.
mod layout;
pub mod path;
pub mod selection;
pub mod style;

/// The font crate winkin is built on, at the version winkin uses.
pub use fontwich;

pub use build::{
    BoxSize, BuildOptions, BuildReport, Clear, FloatSide, LayoutBuilder, OriginalDisplay,
};
pub use context::{CacheLimits, Context, ContextHeap};
pub use layout::paint;
pub use layout::{
    Annotation, Atomic, BoxFragment, Cluster, CrossExtents, FloatPlacement, FontInstance, Glyph,
    InlineEdges, Item, Layout, LayoutHeap, LayoutMetrics, Line, LineMetrics, StaticPosition,
    TextRun,
};
pub use stages::analysis::{BidiLevel, RunOrientation};
pub use stages::content::NodeKey;
pub use stages::fonts::{Generated, NormalizedCoord, SharedFonts};
pub use stages::lines::{
    Area, BlockExtents, Exclusions, ExclusionsCheckpoint, FloatRequest, InlineExtents,
    NoExclusions, PlacedFloat,
};
pub use stages::measure::IntrinsicSizes;
pub use style::{ComputedBlockStyle, ComputedStyle};

// The pipeline --------------------------------------------------------------

// The stages, content to line layout, and the borrowed views of their data
// that breaking, line layout and the readers take.
mod stages;

// Foundations ---------------------------------------------------------------

// Typed ids and the tables they index, the text position type, and the one
// hash, sort and heap accounting.
mod data;
// The fixed-point length types, and the only casts between a float and an
// integer.
mod unit;
// Packed Unicode property tables, segmenters, normalization and the Unicode
// bidirectional algorithm.
mod unicode;
// A count of the steps each stage takes, which tests hold to the size of its
// input and output.
mod work;
// What the stages' tests share: fonts, a fixture, styles.
#[cfg(test)]
mod tests;
