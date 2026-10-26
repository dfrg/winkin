# How fontwich is built

This overview is for contributors and for anyone deciding whether to adopt
fontwich. It describes the crate's structure. The crate docs describe the
public API.

fontwich finds fonts. It lists installed families, matches a CSS request to
a font, handles a document's `@font-face` rules, and answers fallback as the
platform's browser does. It hands back font bytes. It does not shape or
render text, and it depends on no shaper.

## Layers and collections

A [`Layer`](../src/layer/mod.rs) is one complete set of family names. It is
immutable once published and shared behind an `Arc`. Because a layer lists
every name it has from the start, a failed lookup is a real miss, and no
cache of misses is needed. Lookup is a binary search over a sorted index of
names and aliases, compared without allocating.

Each layer has a `Role`:

- `System`: the platform's fonts, from `Layer::system`.
- `Application`: fonts the program supplies.
- `Document`: one document's `@font-face` rules.

A [`Collection`](../src/collection.rs) is a stack of layers. Name lookup
searches from the top, so a document family shadows an installed family of
the same name, as CSS requires. Fallback reads the system layers, or the
application layers when there is no system layer. It never reads document
layers. A collection is cheap to clone and to send to another thread.

A [`LayerBuilder`](../src/layer/builder.rs) owns a layer and edits it by copy on
write. `LayerBuilder::snapshot` publishes the current state, and later edits
never change a snapshot someone else holds. Every edit increments the
layer's generation, which a cache compares to decide whether to lay out
again. Unchanged families keep their records, so a `Family` handle stays
equal across snapshots until its own family changes.

```text
Collection          (stack, searched top down for names)
  Document layer    @font-face faces: names and matching only
  Application layer fonts from bytes, files or directories
  System layer      platform listing + fallback backend
       |
       +-- Family   name, aliases; fonts load on first use
             +-- Font      attributes, axes, charset; bytes load on demand
```

## Families and fonts

A [`Family`](../src/family.rs) is a handle: the layer's `Arc` and an index
into its families. Handles compare and hash by the family record's address,
so they work as cache keys.

A platform layer lists names first and holds a `LoadFamily` loader. A
family's fonts load the first time anything asks for them, once, and other
families stay unloaded. Loading reads metadata only: names, attributes,
axes and the [`Charset`](../src/font/charset.rs) of mapped characters.
Fonts in a layer that map the same characters share one charset.

A [`Font`](../src/font/mod.rs)'s bytes have a separate lifetime. A font's
`Source` is bytes in memory, a file path, or a pending face with no data.
`Font::load` returns [`FontBytes`](../src/font/bytes.rs), shared bytes with
a stable id. For a file, the layer keeps only a `Weak` reference to the
loaded bytes ([`load.rs`](../src/font/load.rs)). Every font naming the file
shares them while any caller holds them, and they are freed when the last
caller drops them. `Font::key` gives the id before anything is read.

## CSS matching

[`matching.rs`](../src/font/matching.rs) implements CSS Fonts 4 font
matching within one family: width first, then style, then weight, each
narrowing the candidates for the next. Each font gets a width, a style and
a weight rank, and the match is the least of them in that order, in one
pass over the family without allocating. Variable fonts match by axis
range. An italic request with no italic font aims at a 14° oblique.
`Font::synthesis` reports the variation settings, synthetic
bold and slant that the matched font needs for the request.

A document layer holds faces. `LayerBuilder::add_face` declares a face from
its descriptors, with or without data. A pending face matches on its
descriptors and maps no characters; `Font::wants` tells the caller to fetch
it, and `LayerBuilder::load_face` supplies the bytes. For `src: local(...)`,
`Collection::local` finds an installed font by its full or PostScript name,
and `LayerBuilder::add_face_font` declares a face over it.

## Fallback

Fallback starts where the caller's `font-family` list runs out.

- **Keys.** A [`FallbackRequest`](../src/fallback/key.rs) describes text in
  a script and language, a generic family, the default Standard font, or
  emoji. `Collection::key` reduces it to a `FallbackKey` that keeps only
  what the collection's backends read. Equal keys have equal answers, and
  the set of keys is finite.
- **Backends.** A [`Backend`](../src/backend.rs) names a key's families, best
  first, as strings. The backends live in [`src/platform/`](../src/platform/mod.rs):
  Windows uses fontwich's tables of Windows fonts by script and language, and DirectWrite's system fallback
  for characters they miss; macOS and iOS ask Core Text; Linux and the BSDs run one fontconfig
  sort per language; Android reads `fonts.xml`. A layer can carry a
  [`FallbackOverride`](../src/fallback/mod.rs), asked before its backend.
  Without the `system` feature, an override is the only source of fallback.
- **The cache.** `Collection::fallback` resolves the names to installed
  families and caches the list by key
  ([`cache.rs`](../src/fallback/cache.rs)). The lock is held only to look up
  or insert, never while a backend runs; two threads that miss the same key
  both compute it, and the first answer stands. The cache is stamped with
  the fallback layers' addresses and generations, and clears itself when
  they change. Pushing a layer that fallback reads gives the collection a
  new cache; pushing a document layer keeps it.
- **Missed characters.** `Collection::char_fallback` returns a lazy iterator
  over the families to try for one character: the character's own key, the
  Common key, the platform's per-character answer, then every other family
  in the fallback layers. It allocates nothing and resolves each key only
  when the walk reaches it.

## Concurrency

Every read takes `&self`. The [`sync`](../src/sync.rs) module provides a
lock and a one-time cell that block under `std` and spin without it. A
thread waiting for a family another thread is loading, or a file another
thread is reading, sleeps under `std`. The fallback cache always uses a spin
lock, since its critical section is a hash probe.

## no_std and features

With no features, fontwich is `no_std` with `alloc`. It holds fonts given
as bytes, supports `@font-face` and matching, and takes fallback from a
`FallbackOverride`.

| Feature | Adds |
| --- | --- |
| `system` (default) | Platform enumeration and fallback: DirectWrite, Core Text, fontconfig. Enables `std`. |
| `std` | Fonts read from files, and directory scanning. |
| `mmap` | Memory-mapped font files. Enables `std`. |
| `android` | The `fonts.xml` backend on any target, so its tests run on any host. Enables `std`. |

On Linux and the BSDs, libfontconfig is linked by default. Setting
`RUST_FONTCONFIG_DLOPEN` at build time loads it at run time instead. This
is an environment variable rather than a feature, because features unify
across a build and would change how other crates reach fontconfig.

## How winkin uses it

[winkin](../../winkin) holds a `Collection` in its `Context`. During font
selection it walks each `font-family` list, looks each name up with
`Collection::family` or `Collection::resolve`, and ranks each family's
fonts with `Family::matching_indices`. It asks `Collection::fallback` for the Standard font,
emoji and the run's script, and `Collection::char_fallback` for characters
still unmapped. It loads the chosen font's bytes with `Font::load` and
applies `Font::synthesis`. Pending faces that would draw the text are
reported to the host. fontwich does not depend on winkin or on any shaper, and
other layout engines can use it the same way.
