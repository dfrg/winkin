# Changelog

All notable changes to cleft are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate
follows [Semantic Versioning](https://semver.org/).

## 0.1.0

The first release.

- Character properties of Unicode 18.0.0: `core_props`, `rare_props` and
  `line_break`.
- Extended grapheme clusters (`Graphemes`, `GraphemeState`) and word
  boundaries (`WordBreaks`), as UAX #29 defines them.
- Line break opportunities (`LineBreaks`), as UAX #14 defines them, with the
  tailorings of CSS's `line-break` and `word-break`.
- Word dictionaries for Thai, Lao, Khmer, Myanmar, and Chinese and Japanese,
  loaded at run time or compiled in with the `dictionaries` feature.
- Full case mapping with language tailorings, with the `case` feature.
- The character classes CSS defines, with the `css` feature.
