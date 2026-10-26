# Changelog

All notable changes to winkin are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate
follows [Semantic Versioning](https://semver.org/).

## 0.1.0

The first release.

- CSS inline layout of one block: styled text, inline boxes, atomic
  inlines, floats, absolutely positioned boxes, forced breaks and ruby,
  built with `LayoutBuilder` and broken into lines with
  `Layout::break_lines`.
- White space processing, bidi, font selection and fallback, shaping, line
  breaking, hyphenation through a host `Hyphenator`, alignment,
  justification and vertical writing modes.
- Lines of positioned glyphs and boxes, paint operations in paint order,
  ruby annotations, and the metrics a host block layout reads.
- Carets, hit testing, motion, selection rectangles and copy
  (`selection`), and text set along paths (`path`).
- `Config`, with presets per platform and `Config::spec()`.
- `no_std` support with `alloc`, and the `std` and `dictionaries` features.
