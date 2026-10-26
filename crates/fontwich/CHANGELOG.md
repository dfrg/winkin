# Changelog

All notable changes to fontwich are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate
follows [Semantic Versioning](https://semver.org/).

## 0.1.0

The first release.

- Font collections built from layers of system, application and document
  fonts (`Collection`, `Layer`, `LayerBuilder`), with family lists read
  without loading font metadata.
- CSS font matching (`Family::match_font`, `Family::matching`) and font
  synthesis (`Font::synthesis`).
- `@font-face` faces declared before their bytes arrive (`FaceDescriptors`,
  `LayerBuilder::add_face`, `Font::wants`).
- Font fallback by script, language and generic family, and per character,
  on DirectWrite, Core Text, fontconfig and Android's `fonts.xml`, with
  `FallbackOverride` for hosts without a platform backend.
- `no_std` support with `alloc`, and the `std`, `system`, `mmap` and
  `android` features.
