# winkin

CSS inline layout for one block.

Lays out styled text, inline boxes, atomic inlines, floats, absolutely
positioned boxes, forced breaks and ruby. Provides positioned glyphs and
boxes, paint order, carets, hit testing and selection geometry.

`Config` holds the choices CSS leaves open or where implementations
differ, with presets per platform; `Config::spec()` follows the CSS
specifications throughout. The host supplies computed styles, font loading,
float placement, block layout and painting.

See [the CSS winkin supports](docs/css-support.md) and
[the showcase](docs/showcase.md) of its output.

## Usage

Build content with `LayoutBuilder`, call `break_lines` for the available
width, and read the results. Repeated line breaking reuses prepared content.
Fonts come from [fontwich](../fontwich).

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
let mut builder = layout.builder(NodeKey(0), &block, BuildOptions::default());
builder.text(NodeKey(1), "Hello, world");
builder.finish(&mut cx);

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

Use `open_box` and `close_box` for inline boxes, `text` for text nodes, and `atomic`
and `float` for boxes already laid out by the host, and `absolute` for
absolutely positioned boxes, which take no room. Break methods represent
`<br>`, `<br clear>` and `<wbr>`. Ruby and first-letter methods supply
annotations and pseudo-element styles. `BuildReport` records content dropped
or replaced when layout limits are exceeded.

Additional APIs:

- `Line::paints`: paint order and geometry, including font decoration metrics.
- `Line::annotations`: ruby text runs and atomic inlines.
- `Layout::metrics`, `intrinsic_sizes`, `room_below` and `has_annotations`:
  host block measurements.
- `Layout::measure`: new sizes for atomic inlines and floats, and a new
  basis for percentage margins and padding, without building again.
- `Layout::size_lines`: lines and block metrics for sizing a block, without
  positioning items.
- `Layout::floats` and `static_positions`: where floats went, and each
  absolutely positioned box's static position.
- `selection`: carets, hit testing, motion, selection rectangles and copy.
- `path`: text placement along curves, matching SVG `textPath`.
- `config`: platform presets and behavior beyond CSS properties.
- `font::FontMetricsProvider`: optional host glyph metrics.

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
Blitz's WPT runner, winkin passes 3,548 of 5,078 tests; Chrome 153 passes
4,396.

## Features

- `std` (default): passes `std` on to fontwich, read-fonts and harfrust.
  Without it the crate is `no_std` with `alloc`.
- `dictionaries` (default): ICU's dictionaries segment Thai, Lao, Khmer,
  Myanmar, Chinese and Japanese. Without it, an LSTM model
  segments the Southeast Asian scripts and Unicode's word rules find Chinese
  and Japanese words, and the binary is about 3.5 MB smaller.

## Read next

- [`docs/css-support.md`](docs/css-support.md): the CSS winkin supports, the
  `Config` fields, and what the host does.
- The crate docs: the public API, starting at `LayoutBuilder` and `Layout`.
- [`docs/architecture.md`](docs/architecture.md): how winkin is built.

## The name

winkin pays its respects to Wynkyn de Worde, England's most prolific early
printer, and to Minikin, Android's layout engine. Half typesetter, half
robot. It also sounds like something you'd do at a well-justified paragraph.

## Licence

Licensed under either of the Apache License, Version 2.0, or the MIT licence,
at your option.
