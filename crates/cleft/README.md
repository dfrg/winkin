# cleft

Unicode character properties, and grapheme, word and line segmentation.

cleft reads its own compact tables, generated from the Unicode Character
Database. It is `no_std`, has no dependencies, and no lookup or scan
allocates or panics, whatever the input.

- **Properties.** `core_props` and `rare_props` return a character's
  properties in one table lookup each: the bidi class, the script and its
  extensions, paired brackets, emoji and break-related flags,
  Grapheme_Cluster_Break, East_Asian_Width, Vertical_Orientation and the
  casing properties.
- **Segmentation.** `Graphemes` finds extended grapheme clusters and
  `WordBreaks` word boundaries (UAX #29), and `LineBreaks` finds line break
  opportunities (UAX #14) under the tailorings of CSS's `line-break` and
  `word-break`. Each reads UTF-8 or UTF-16 text.
- **Dictionaries.** Thai, Lao, Khmer, Myanmar, Chinese and Japanese break at
  the words of a dictionary, compiled in or loaded at run time.

```rust
use cleft::{Graphemes, LineBreaks, LineOptions, Utf8Scalars};

// The offsets where a line may break.
let text = "Can't stop, 3.14!";
let breaks: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), LineOptions::new()).collect();
assert_eq!(breaks, [6, 12]);

// The offsets where grapheme clusters start.
let text = "a\u{308}🇫🇷!";
let starts: Vec<usize> = Graphemes::new(Utf8Scalars::new(text)).collect();
assert_eq!(starts, [0, 3, 11]);
```

## Features

No feature is on by default.

| Feature | Adds |
|---|---|
| `css` | The character classes CSS defines and Unicode does not, for `text-autospace`, `text-spacing-trim`, `text-emphasis-skip` and `::first-letter`. |
| `case` | Full case mapping with language tailorings: `uppercase`, `lowercase` and `titlecase`. |
| `dictionaries` | The word dictionaries of Thai, Lao, Khmer, Myanmar, and Chinese and Japanese, compiled in (about 2.7 MB). |

## Unicode version

The tables and rules are of Unicode 18.0.0, which `UNICODE_VERSION` names.

## Minimum Rust version

cleft builds with Rust 1.88 and later.

## License

cleft's code is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Its property and case tables are generated from the Unicode Character
Database, under the Unicode License v3 ([LICENSE-UNICODE](LICENSE-UNICODE)).
Its dictionaries are generated from ICU's word lists, under ICU's license
and the notices of the lists' sources, reproduced in
[THIRD-PARTY](THIRD-PARTY).
