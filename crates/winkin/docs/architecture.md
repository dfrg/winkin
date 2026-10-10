# How winkin is built

This overview is for contributors and for anyone deciding whether to adopt
winkin. It describes the crate's structure. The crate docs describe the
public API.

## The model

winkin lays out one CSS inline formatting context: the lines of one block.

A host builds the content once with [`LayoutBuilder`](../src/build.rs). The
builder takes text, inline boxes, atomic inlines, floats, absolutely
positioned boxes, forced breaks and ruby, in document order, each with its
computed style. `LayoutBuilder::finish` prepares everything that does not
depend on the available width. `Layout::break_lines` then breaks the content
into lines for one width. Preparing runs once per content change; breaking
runs once per width, and a second width repeats none of the preparation.
`Layout::measure` gives the boxes new geometry without building again: new
sizes for atomic inlines and floats, and a new width for percentage margins
and padding. It keeps the analysis, fonts and shaping and measures again from
the shaped advances, or, where only atomic inlines' block sizes and baselines
change, measures again only their extents.

The host reads the result through views on [`Layout`](../src/layout/mod.rs):
lines and the items on them, paint operations, box fragments, floats and
static positions, block metrics, [selection](../src/selection/mod.rs) and
[text on paths](../src/path/mod.rs). Reading needs no context, does not
allocate, and can happen on another thread.

## The pipeline

The work runs as a series of stages. A stage is one step of the pipeline
together with the tables it writes. Each stage writes only its own tables,
and they are frozen once the stage returns. Later stages borrow them
immutably, so the borrow checker enforces the order.

```text
build    LayoutBuilder calls   -> content: text, nodes, items, style facts
prepare  analysis              -> clusters, breaks, paragraphs, bidi, script runs
         fonts                 -> used fonts and font runs
         shape                 -> glyphs and advances
         measure               -> advance prefix sums, intrinsic sizes, line-edge costs
break    lines                 -> the lines for one width, and the block's metrics
         fragments             -> positioned items in visual order
read     Layout::lines, Line::paints, selection, path
```

Every stage lives in `src/stages/<stage>/`. Its `mod.rs` holds the entry
function, its input and scratch types, and a module doc that says what goes
in, what comes out and where to read next.

- **[content](../src/stages/content/mod.rs).** In: the builder's calls. Out:
  `Content`: the text after white space collapsing and `text-transform`, the
  nodes, the items in reading order, and the interned style facts. It reads
  no font and no width. Start at `ContentWriter::new`, then
  [`writer_calls.rs`](../src/stages/content/writer_calls.rs).
- **[analysis](../src/stages/analysis/mod.rs).** In: the content. Out:
  `Analysis`: grapheme clusters, line break opportunities, paragraphs, bidi
  levels, and script and orientation runs. Start at `analyze`, then
  `ClusterWriter::visit_item` in [`walk.rs`](../src/stages/analysis/walk.rs).
- **[fonts](../src/stages/fonts/mod.rs).** In: the content and the analysis.
  Out: `Fonts`: a used font for every cluster, as font runs, and the table of
  used fonts with their line metrics. Start at `select_fonts`, then
  `select_runs` in [`select.rs`](../src/stages/fonts/select.rs).
- **[shape](../src/stages/shape/mod.rs).** In: the content, analysis and
  fonts. Out: `Shaped`, the glyphs of each shaping run, and the advances,
  handed to measurement by value. Shaping calls
  [harfrust](https://github.com/harfbuzz/harfrust). Start at `shape_runs`,
  then `shape_text` in [`walk.rs`](../src/stages/shape/walk.rs).
- **[measure](../src/stages/measure/mod.rs).** In: everything before it, and
  the advances. Out: `Measured`: advance prefix sums, intrinsic sizes, item
  extents, line-edge costs and ruby columns. Start at `measure_text`, then
  `Scan::walk` in [`scan.rs`](../src/stages/measure/scan.rs).
- **[lines](../src/stages/lines/mod.rs).** In: every prepared table, the
  area and the host's exclusions. Out: `Lines`, each line written whole when
  it is chosen, and the block's result. Edges that the font shaped across are
  reshaped for the line, so a line's width is exact. Start at `break_lines`,
  then `Breaker::lines` in [`fit.rs`](../src/stages/lines/fit.rs).
- **[fragments](../src/stages/fragments/mod.rs).** In: every prepared table
  and the lines. Out: `Fragments`, one flat table of positioned items per
  block, each line's items in visual order after alignment, justification
  and bidi reordering. Start at `place_fragments`, then `Placer::lines` in
  [`place.rs`](../src/stages/fragments/place.rs).

Both drivers are in [`layout/mod.rs`](../src/layout/mod.rs):
`PreparedStages::prepare` runs analysis to measurement, and
`Layout::break_lines` runs line breaking and then line layout.

A block's first line can be restyled by `::first-line`. Where that changes
shaping or measurement, the shape and measure stages also write a first-line
version, and [`stages/mod.rs`](../src/stages/mod.rs) picks the version once
per line.

## Data shape

**Typed ids into row tables.** Stage data is tables of rows. An `XId`
indexes a table of `X` and nothing else; ids are newtypes over `u32` or
smaller, never `usize`. The primitives are in
[`data/mod.rs`](../src/data/mod.rs): `Table`, `Runs` for runs that tile a
range of clusters, bit tables, and the bounded `LruCache`.

**Style is lowered at the builder.** winkin keeps no `ComputedStyle`. The
builder lowers each style into a few kinds of facts, each interned once by
value: a `FontRequest`, `ShapingFacts` for text that shapes alike,
`TextFacts` for the other inherited properties, and `BoxFacts` for a box.
Later stages read the facts, never the style. Reusing a style costs one
lookup. See [`stages/content/facts.rs`](../src/stages/content/facts.rs).

**Walks carry their context.** The runs form one hierarchy: paragraph, item,
script run, font run, shaping run, line. A stage that walks clusters walks
[`Segments`](../src/stages/segments.rs): ranges of clusters inside one
paragraph, one item and one shaping run, carrying their ids. A function
handed a single position takes a `Slot`, the cluster with its item and run.
A rule asked during a walk reads the rows it needs by id and never searches
for them. Searching by cluster or offset happens only where a random-access
read starts, such as a hit test or a caret.

**Fixed-point units.** Lengths use the fixed-point types in
[`unit.rs`](../src/unit.rs): `TextUnit` (16.16) for glyph geometry,
`LayoutUnit` (26.6) for layout geometry, and `InlineLayoutUnit` (48.16) for
running advances. These are the types Chrome uses, rounded where Chrome
rounds. Values enter and leave as `f32` pixels, and every conversion between
a float and an integer is in `unit.rs`.

## Context and Layout

A [`Context`](../src/context.rs) holds the font `Collection`, the `Config`,
the caches shared across layouts, and the working memory of each stage. The
caches are least-recently-used tables bounded by `CacheLimits`: family
lists, fallback lists, font instances and shape plans. The context trims
them at the start of every build and every line break, and releases scratch
much larger than recent calls needed. Use one context per thread.

A `Layout` owns its content, its prepared stages, its lines and its
fragments. Its used fonts hold `Arc`s to the font instances and their bytes,
never ids into the context's caches. So the context can drop any cache entry
between calls, and a layout stays valid, readable and breakable after
`Context::clear_caches` or a new collection.

## What the host supplies

- Computed styles, as [`ComputedStyle`](../src/style/mod.rs) and
  `ComputedBlockStyle`. The host runs the cascade and keeps paint-only
  properties, which it looks up by `NodeKey` when painting.
- Fonts, as a fontwich `Collection`. winkin does CSS font matching and
  platform fallback through it.
- Float placement, through the [`Exclusions`](../src/stages/lines/exclusions.rs)
  trait. `NoExclusions` serves a block with no floats.
- Block layout: the sizes of atomic inlines and floats, which the host lays
  out first, and the use of `Layout::metrics`, `intrinsic_sizes` and
  `static_positions`.
- Painting. `Line::paints` yields operations in CSS paint order; the host
  draws them.
- Optionally, hinted glyph metrics through
  [`FontMetricsProvider`](../src/font.rs), passed to
  `LayoutBuilder::finish_with_metrics` and `Layout::break_lines_with_metrics`.

## Behavior

[`Config`](../src/config.rs) holds the choices that CSS properties do not
decide, or where implementations differ, with presets for Windows, macOS
and Linux. `Config::spec()` follows the CSS specifications throughout. A
choice that can vary from one call
to the next is an argument to that call instead, such as `PastLines` for hit
testing and `WordMotion` for word movement. [`css-support.md`](css-support.md)
lists every property winkin reads.

Building never panics and never fails. Content beyond a limit is dropped
whole and counted in the `BuildReport`.

## Testing

- **Unicode conformance.** Clusters, line breaks, bidi and normalization are
  checked against Unicode's own test files, and the packed property tables
  against ICU for every character.
- **Allocation and size.** [`tests/allocations`](../tests/allocations/main.rs)
  checks that every stage allocates nothing once warm.
  [`tests/heap.rs`](../tests/heap.rs) checks that heap accounting matches
  the allocator. Each stage has a test that pins the sizes of its records.
- **Work counters.** In test builds, each loop whose length depends on input
  counts its steps, and each search counts itself
  ([`work.rs`](../src/work.rs)). Tests build the same content at two sizes
  and compare the counts, so a stage that grows faster than its input, or
  searches where it should walk, fails without depending on timing.
- **Behavior.** Each stage's `tests/` folder holds tests with expected
  values from Chrome or the CSS specifications.
