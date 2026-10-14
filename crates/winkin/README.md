# winkin

CSS inline layout for one block.

Lays out styled text, inline boxes, atomic inlines, floats, absolutely
positioned boxes, forced breaks and ruby. Provides positioned glyphs and
boxes, paint order, carets, hit testing and selection geometry.

`Config` holds the choices CSS leaves open and those where implementations
differ, with a preset per platform; `Config::spec()` follows the CSS
specifications throughout. The host supplies computed styles, font loading,
float placement, block layout and painting.

See [the CSS winkin supports](docs/css-support.md) and
[the showcase](docs/showcase.md) of its output.

## Usage

Build content with `LayoutBuilder`, call `break_lines` for the available
width, and read the results. Breaking lines again reuses the prepared
content. Fonts come from [fontwich](../fontwich).

```rust,ignore
use fontwich::{Collection, LayerBuilder, Role};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Item, Layout, NoExclusions,
    NodeKey,
};

// Add a font from bytes, or use `Collection::system()` with
// the fontwich `system` feature for installed fonts.
let mut fonts = LayerBuilder::new(Role::Application);
fonts.add_data(font_bytes)?;
let mut cx = Context::new(Collection::new().with_layer(fonts.snapshot()));

let mut layout = Layout::new();
let style = ComputedStyle::initial();
let block = ComputedBlockStyle::new(&style);
let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
builder.text(NodeKey(1), "Hello, world");
builder.finish();

layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);

for line in layout.lines() {
    let metrics = line.metrics();
    for item in line.items() {
        if let Item::Text(run) = item {
            let Some(font) = run.font() else { continue };
            for glyph in run.glyphs() {
                // Draw glyph.id from `font` at
                // (metrics.left + glyph.x, metrics.top + glyph.y).
            }
        }
    }
}
```

The builder takes `text` for text nodes, `open_box` and `close_box` for
inline boxes, `atomic` and `float` for boxes the host has laid out, and
`absolute` for absolutely positioned boxes, which take no room.
`line_break`, `line_break_clearing` and `break_opportunity` stand for
`<br>`, `<br clear>` and `<wbr>`. The ruby methods add bases and
annotations, and `set_first_letter` styles `::first-letter`. A build never
fails: `BuildReport` counts content dropped or replaced at a layout limit.

Additional APIs:

- `Line::paints`: paint order and geometry, including font decoration metrics.
- `Line::annotations`: ruby text runs and atomic inlines.
- `Layout::metrics`, `intrinsic_sizes`, `room_below` and `has_annotations`:
  host block measurements.
- `Layout::measure`: new sizes for atomic inlines and floats, and a new
  basis for percentage margins and padding, without building again.
- `Layout::set_text_align`: new `text-align` and `text-align-last`, without
  building again.
- `Layout::size_lines`: lines and block metrics for sizing a block, without
  positioning items.
- `Layout::floats` and `static_positions`: where floats went, and each
  absolutely positioned box's static position.
- `selection`: carets, hit testing, motion, selection rectangles and copy.
- `path`: lines set along curves, as SVG `textPath` sets them.
- `config`: platform presets and behavior beyond CSS properties.
- `font::FontMetricsProvider`: optional host glyph metrics.
- `SharedFonts` and `Context::with_shared_fonts`: contexts that parse each
  font once between them.

## Examples

- [`examples/paragraph.rs`](examples/paragraph.rs) lays out a justified
  paragraph with a bold span and prints its lines and glyph runs:
  `cargo run -p winkin --example paragraph`.
- [`examples/advance_cache.rs`](examples/advance_cache.rs) shows a host
  supplying hinted glyph advances through `FontMetricsProvider`.

The crate docs (`cargo doc -p winkin --open`) have a runnable version of the
example above.

## Status

winkin is at 0.0.1 and not yet published to crates.io. The API may change.

[`docs/css-support.md`](docs/css-support.md) lists every CSS property winkin
reads, how far each goes, and what the host does. On the web platform tests'
inline-text suites (css-text, css-writing-modes, css-fonts, css-text-decor,
css-inline, css-ruby, and CSS 2's text and line box tests), run through
Blitz's WPT runner, winkin passes 4,182 of 5,356 tests; Chrome 155 passes
4,393.

## Features

- `std` (default): passes `std` on to fontwich, read-fonts and harfrust.
  Without it the crate is `no_std` with `alloc`.
- `dictionaries` (default): ICU dictionaries segment Thai, Lao, Khmer,
  Myanmar, Chinese and Japanese. Without it, an LSTM model segments the
  Southeast Asian scripts, Unicode word rules find Chinese and Japanese
  words, and the binary is about 3.5 MB smaller.

## Read next

- [`docs/css-support.md`](docs/css-support.md): the CSS winkin supports, the
  `Config` fields, and what the host does.
- The crate docs: the public API, starting at `LayoutBuilder` and `Layout`.
- [`docs/architecture.md`](docs/architecture.md): how winkin is built.

## The name

winkin is named for Wynkyn de Worde, England's most prolific early printer,
and for Minikin, Android's layout engine. Half typesetter, half robot. It
also sounds like something you'd do at a well-justified paragraph.

## Licence

Licensed under either of the Apache License, Version 2.0, or the MIT licence,
at your option.
