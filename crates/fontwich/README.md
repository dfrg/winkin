# fontwich

Font enumeration, CSS matching and platform fallback.

Lists font families without loading metadata, matches fonts to CSS requests,
and supports document `@font-face` rules. Fallback uses the platform font
APIs on Windows, macOS and Linux, and reads Android's `fonts.xml`.

- `Collection` searches layers from top to bottom. Layers contain system,
  application or document fonts.
- `Family` loads metadata on first use. `Font` loads bytes separately.
- Fallback caches installed families by request key. Character fallback
  searches additional families when needed.
- Matching resolves generic family aliases and ranks oblique angles
  against a 14° target for italic requests. `Font::synthesis` provides variation settings, synthetic bold and slant.
- Name lookup and cached fallback traversal do not allocate.

The crate provides font bytes but does not shape or render text.
[winkin](https://crates.io/crates/winkin) uses it for font selection during layout.

## Example

```rust
use fontwich::{Collection, FallbackRequest, GenericClass, Presentation, Script, parse_language};

// List installed families without loading their fonts.
let fonts = Collection::system();

// Look up a family by name, in any case.
if let Some(arial) = fonts.family("arial") {
    println!("{} has {} fonts", arial.name(), arial.fonts().len());
}

// Ask fallback for Arabic text.
let request = FallbackRequest::Text {
    script: Script::from_bytes(*b"Arab"),
    language: parse_language("ar"),
    generic: GenericClass::Plain,
};
for family in fonts.fallback(&fonts.key(&request)).iter() {
    println!("{}", family.name());
}

// Find a fallback family for an unsupported character.
let found = fonts
    .char_fallback('﷽', Presentation::Text, &request)
    .find(|family| family.covers('﷽'));
```

The crate docs (`cargo doc -p fontwich --open`) have more examples:
`@font-face` faces, matching and synthesis, and character coverage.
`cargo run -p fontwich --example dump` prints the platform's fallback
families for a few representative scripts.

## Status

fontwich is at 0.1.0. The API may change between minor versions. On
Android, fontwich reads `fonts.xml` but not yet its `variant` attribute.

## Features

| Feature | What it adds |
| --- | --- |
| `system` (default) | Native font enumeration and fallback: DirectWrite, Core Text, fontconfig. Enables `std`. |
| `std` | Fonts read from files, and directory scanning. |
| `mmap` | Memory-mapped font files. Enables `std`. |
| `android` | The `fonts.xml` backend, on any target. Enables `std`. |

With no features, fontwich is `no_std` with `alloc`. It takes fonts as
bytes, and a `FallbackOverride` supplies fallback.

On Linux and the BSDs, libfontconfig is linked by default. Set
`RUST_FONTCONFIG_DLOPEN` at build time to load it at run time instead.

## Minimum Rust version

fontwich builds with Rust 1.88 and later.

## Read next

- The crate docs: the public API, starting at `Collection`.
- [docs/architecture.md](docs/architecture.md): how fontwich is organized.

## The name

fontwich is a sandwich of font layers — system on the bottom, your app in
the middle, the document's `@font-face` on top — and its whole job is
answering *which font?* Hold the mayo.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Some tables record facts from Chromium's default font settings and font
lists, and from the Unicode Character Database. Their notices are in
[THIRD-PARTY](THIRD-PARTY).
