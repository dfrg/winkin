# winkin

CSS inline layout in Rust, and the font crate it uses.

See [the CSS winkin supports](crates/winkin/docs/css-support.md) and
[the showcase](crates/winkin/docs/showcase.md) of its output.

## Crates

| Crate | Description |
|---|---|
| [`winkin`](crates/winkin) | The layout engine. Takes styled text, inline boxes, atomic inlines, floats, absolutely positioned boxes and ruby; produces lines of positioned glyphs and boxes, paint order, carets, hit testing and selection. |
| [`fontwich`](crates/fontwich) | Font enumeration, CSS font matching, `@font-face` and platform font fallback on Windows, macOS, Linux and Android. Usable on its own. |

The workspace also contains unpublished crates:

| Path | Description |
|---|---|
| [`support/testing`](support/testing) | Test fonts, in-memory fonts, an allocation counter and Unicode's conformance files for winkin's tests. |
| [`support/unicode_gen`](support/unicode_gen) | The generator for winkin's Unicode property tables. |
| [`examples/editor`](examples/editor) | A plain-text editor rendered with Vello CPU, with selection, clipboard, undo and IME input. |

## Using winkin

A host builds a layout's content once, breaks it into lines at a width, and
reads the lines back. A relayout at another width breaks it again; nothing
prepared is done twice.

```rust,ignore
use fontwich::{Collection, LayerBuilder, Role};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Item, Layout, NoExclusions,
    NodeKey,
};

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
    for item in line.items() {
        if let Item::Text(run) = item {
            // Draw run.glyphs() in run.font().
        }
    }
}
```

[`crates/winkin/README.md`](crates/winkin/README.md) says more, and
[`crates/winkin/docs/css-support.md`](crates/winkin/docs/css-support.md)
lists every CSS property winkin reads and how far each goes.

## Status

Both crates are at 0.0.1 and not yet published to crates.io. The APIs may
change.

On the web platform tests' inline-text suites (css-text, css-writing-modes,
css-fonts, css-text-decor, css-inline, css-ruby, and CSS 2's text and line
box tests), run through Blitz's WPT runner, winkin passes 3,548 of 5,078
tests; Chrome 153 passes 4,396. Most of the difference is block layout in
vertical writing modes, which belongs to the host.

## Build and test

```text
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p winkin --example paragraph
```

CI runs the full set of checks in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml).

## How winkin is built

A layout is prepared once per content change and broken into lines once per
width. Each stage writes one set of tables that later stages only read.

```text
build      LayoutBuilder calls        -> content: text, nodes, items, style facts
prepare    analysis::analyze          -> clusters, paragraphs, bidi, breaks, runs
           fonts::select_fonts        -> used fonts and font runs
           shape::shape_runs          -> glyphs
           measure::measure_text      -> prefix advances, line-end costs, heights
break      lines::break_lines         -> lines and the block's metrics
           fragments::place_fragments -> placed fragment items
read       Layout::lines, Line::paints, selection, path
```

Each stage's entry is in its `mod.rs` under `crates/winkin/src/stages/`,
whose module doc says what goes in, what comes out and where to read next.
Both orchestrators are in `crates/winkin/src/layout/mod.rs`:
`PreparedStages::prepare` runs the preparation stages, and
`Layout::break_lines` runs breaking and line layout.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
