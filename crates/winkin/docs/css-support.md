# CSS support in winkin

winkin lays out one CSS inline formatting context: the inline content of
one block, broken into lines. Text, nested inline boxes, atomic inlines,
floats, absolutely positioned boxes, forced breaks and ruby go in through a
builder. Lines of positioned glyphs, boxes and marks come out, with what
each line paints, in the order Chrome paints it.

This page lists the CSS that winkin reads, how far each feature goes, and
what the host does itself. It is written for developers who put winkin
inside a browser engine, a UI toolkit or a document renderer.

## How to read this page

**Chrome is the reference.** By default winkin does what Chrome does on the
platform it runs on. Where Chrome has a bug or a limitation that CSS does
not allow, winkin does what CSS says, and the notes name the difference.
Where winkin can do better than Chrome, the better behaviour is either a
`Config` choice that is off by default, or a CSS feature that Chrome does
not parse.

**Each feature has a status:**

| Status | Meaning |
|---|---|
| Supported | Implemented. |
| Partial | Works with the limits the notes give. |
| Beyond Chrome | Chrome does not support it, or does less. Winkin supports it. A Config choice is opt-in; a CSS feature works when the style sets it. |
| Not supported | The API has no value for it, or accepts it and ignores it. |

**Values are computed values.** The host runs the cascade and hands winkin a
`ComputedStyle` for each box and a `ComputedBlockStyle` for the block. The
tables list the values those types accept. A keyword that the cascade
resolves to another (`line-break: auto`, `text-align: match-parent`) is the
host's to resolve. Lengths are CSS pixels.

**Config** is `winkin::config::Config`, set on a `Context` with
`set_config`. A context starts with the preset for the platform it is built
for. The presets are `Config::chrome_windows()`, `chrome_mac()` and
`chrome_linux()`. `Config::spec()` follows the CSS specifications wherever
they differ from Chrome, for WPT runs and hosts that want spec behaviour.
See [Config at a glance](#config-at-a-glance).

## Contents

- [White space and text processing](#white-space-and-text-processing)
- [Transforms and case](#transforms-and-case)
- [Line breaking and word breaking](#line-breaking-and-word-breaking)
- [Hyphenation](#hyphenation)
- [Bidi and direction](#bidi-and-direction)
- [Fonts and font selection](#fonts-and-font-selection)
- [Spacing](#spacing)
- [Alignment and justification](#alignment-and-justification)
- [Line height and vertical alignment](#line-height-and-vertical-alignment)
- [Inline boxes](#inline-boxes)
- [Atomic inlines](#atomic-inlines)
- [text-indent](#text-indent)
- [Floats](#floats)
- [Absolutely positioned boxes](#absolutely-positioned-boxes)
- [::first-letter and initial-letter](#first-letter-and-initial-letter)
- [::first-line](#first-line)
- [text-overflow and line-clamp](#text-overflow-and-line-clamp)
- [text-box-trim and text-box-edge](#text-box-trim-and-text-box-edge)
- [text-wrap: balance and pretty](#text-wrap-balance-and-pretty)
- [Ruby](#ruby)
- [Emphasis marks](#emphasis-marks)
- [Vertical writing](#vertical-writing)
- [Selection and hit testing](#selection-and-hit-testing)
- [Text on paths](#text-on-paths)
- [Not supported](#not-supported)
- [What the host does](#what-the-host-does)
- [Config at a glance](#config-at-a-glance)
- [How this is tested](#how-this-is-tested)

---

## White space and text processing

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `white-space-collapse` | `collapse`, `preserve`, `preserve-breaks`, `break-spaces` | Supported | Collapsing runs across inline box edges. An atomic inline ends a run of white space. A float does not: white space collapses across it. | — |
| `white-space-collapse` | `preserve-spaces`, `discard` | Beyond Chrome | Chrome parses neither. `preserve-spaces` keeps spaces and tabs and turns a segment break into a space. `discard` removes all white space. | — |
| `text-wrap-mode` | `wrap`, `nowrap` | Supported | Under `nowrap`, a break after a space or a tab follows the style of the box that holds the space. | — |
| `white-space` | shorthand | Supported | The host expands it into `white-space-collapse` and `text-wrap-mode`. | — |
| `white-space-trim` | `discard-before`, `discard-after`, `discard-inner` | Beyond Chrome | Chrome does not implement it. On the block, `discard-inner` also removes kept white space through the first segment break and from the last, as for `<pre>`. Not inherited. | — |
| `tab-size` | a number of spaces, a length | Supported | Tab stops are measured from the content box's start edge. A number counts the block container's space, with its `letter-spacing` and `word-spacing`, as Chrome sizes a tab. At `tab-size: 0` a tab is as wide as the letter-spacing. | `tab_justification` |
| Segment breaks | — | Supported | Under `collapse`, a segment break becomes a space, and is removed beside a U+200B, as Chrome does. | — |
| Trailing white space | — | Supported | Spaces at a line's end hang. Under `pre-wrap` (and `preserve-spaces` with wrapping), white space before a forced break or the block's end hangs only where it overflows. Under `pre` it is content and overflows. Under `break-spaces` it wraps. A space before a hanging U+3000 hangs with it, and U+3000 before preserved spaces ending a paragraph hangs only where they all overflow, as in Chrome. Other space separators (U+2000 to U+200A) hang too, where Chrome hangs none. | — |
| Control characters | — | Supported | As CSS Text 3 has them. A lone CR is a space in every mode. CRLF is one segment break. VT, FF, NEL, U+2028 and U+2029 are forced breaks in every mode and take the collapsible spaces on both sides. VT, FF and NEL draw as glyphs and take room. Chrome drops CR and FF under preserved white space and draws FF with no break under `collapse`: winkin does not copy this. | — |
| `<br>` | — | Supported | `LayoutBuilder::line_break`. It is `\n` in the layout's text and ends the paragraph. Inside ruby it is a space, as in Chrome. | — |
| `<br clear>`, `clear` on a `<br>` | `left`, `right`, `both` | Supported | `LayoutBuilder::line_break_clearing`. The next line, or the block's end, moves below the host's floats on those sides, as Chrome applies a break's clearance after its line. Inside ruby the break is a space and clears nothing, as in Chrome. | — |
| `<wbr>` | — | Supported | `LayoutBuilder::break_opportunity`. It is U+200B in the layout's text. White space collapses across it. | — |
| Default-ignorable characters | — | Supported | Soft hyphens, U+200B, word joiners, U+FEFF, bidi controls and lone joiners are shaped with their text and draw nothing, as Chrome keeps them. A ZWNJ ends a shaping run only under `preserve`, `break-spaces` and `preserve-spaces`, as in Chrome. | — |

## Transforms and case

`text-transform` is applied as the text is written, so every later stage
sees the text as drawn.

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-transform` | `none`, `uppercase`, `lowercase`, `capitalize` | Supported | Full case mappings, in the text's language: Turkish and Azerbaijani dotted and dotless i, Lithuanian dots, Greek accents dropped in capitals, Dutch IJ, final sigma. `capitalize` finds words by Unicode's word boundaries, the same words word motion finds, and titlecases the first letter. | — |
| `text-transform` | `full-width` | Beyond Chrome | Chrome has it behind a flag. Printable ASCII, half-width katakana and Hangul and a few signs become full-width. A space becomes U+3000 only where white space is kept. Applied after the case. | — |
| `text-transform` | `full-size-kana` | Beyond Chrome | Chrome has it behind a flag, with the same table. CSS Text 3's 58 small kana become full-size. Applied after `full-width`. | — |
| `text-transform` | `math-auto` | Supported | `TextTransform::MATH_AUTO`. MathML Core's italic mappings for single-character source text nodes; consecutive calls with the same key form one node. | [MathML Core §4.2](https://w3c.github.io/mathml-core/#the-math-auto-transform) |
| `font-variant-caps` | all values | Supported | See [Fonts](#fonts-and-font-selection). Synthesized small capitals are case transforms done at shaping. | — |

## Line breaking and word breaking

Line break opportunities follow UAX #14 (Unicode 17), with Chrome's own
rules for ASCII first. So, as in Chrome, there is no break after `/`
between letters or digits, and a hyphen before a digit breaks only after a
letter or digit.

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `word-break` | `normal`, `break-all`, `keep-all` | Supported | `keep-all` follows CSS's character classes. Chrome breaks before an emoji under `keep-all`; winkin does not. | — |
| `word-break` | `break-word` | Supported | The host maps it to `word-break: normal` with `overflow-wrap: anywhere`, as CSS defines it. | — |
| `word-break` | `auto-phrase` | Not supported | No value in the API. | — |
| `line-break` | `loose`, `normal`, `strict`, `anywhere` | Supported | The host resolves `auto` to `normal`. The strictness applies even without a `lang`, where Chrome drops it (a Chrome bug, fixed upstream). | `small_kana` |
| `overflow-wrap` (`word-wrap`) | `normal`, `anywhere`, `break-word` | Supported | Both break an overflowing word anywhere, and between two items where either side allows it, as Chrome's retry after an overflow does. `anywhere` also lets min-content break there; `break-word` does not. | — |
| Thai, Lao, Khmer, Myanmar | — | Supported | Broken by ICU's dictionaries, as Chrome breaks them. Without the `dictionaries` Cargo feature, an LSTM model breaks them instead, and the binary is about 3.5 MB smaller. | Cargo feature `dictionaries` (on by default) |
| Chinese and Japanese words | — | Supported | For word motion and `capitalize`: ICU's dictionary with the `dictionaries` feature; UAX #29's own rules without it. Line breaking uses the CJK rules either way. | Cargo feature `dictionaries` |
| Breaks beside atomic inlines | — | Supported | As CSS now says: no break beside a non-breaking glue character other than U+00A0, WJ or ZWJ. Chrome breaks beside every character here, a lag winkin does not copy. | — |
| Small kana | — | Supported | Under `line-break: normal`, Chrome lets a small kana start a line. `Held` holds it to the character before, as `strict` does and as CSS Text now asks. | `small_kana` |

## Hyphenation

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `hyphens` | `none`, `manual` | Supported | Under `none`, a soft hyphen is no break opportunity. Under `manual`, a line may break at a soft hyphen. | — |
| `hyphens` | `auto` | Not supported | Accepted, and treated as `manual`: lines break at soft hyphens only. There is no dictionary hyphenation. | — |
| `hyphenate-character` | `auto`, a string | Supported | `auto` draws U+2010 where the primary font maps it and `-` where it does not, as Chrome does. The string may be empty. The hyphen is set in the style's fonts with fallback, as the text would be. Its width counts where the line is measured, in min-content and in justification. No hyphen shows at the text's end or before a forced break. | — |
| `hyphenate-limit-*` | — | Not supported | | — |

## Bidi and direction

The Unicode Bidirectional Algorithm is winkin's own, conformant to Unicode
17 (it passes BidiTest and BidiCharacterTest).

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `direction` | `ltr`, `rtl` | Supported | Per box. It also decides which side is a box's start, for its edges. | — |
| Block base direction | `Auto`, `Ltr`, `Rtl` | Supported | `ComputedBlockStyle::direction`. `Auto` takes each paragraph's direction from its first strong character, for `dir=auto`. | — |
| `unicode-bidi` | `normal`, `embed`, `isolate`, `bidi-override`, `isolate-override`, `plaintext` | Supported | Applied at the box's edges; no control characters are written into the text. Not inherited. | — |
| Paragraph separators U+001C to U+001E | — | Supported | Each ends a bidi paragraph but not a line. The style's embeddings re-open after it, as CSS Writing Modes 4 asks. Chrome re-opens only at `\n`. | — |
| Line reordering | — | Supported | Trailing preserved spaces reset to the paragraph level, as in Chrome. Collapsible spaces at a line's end are removed. | — |
| `::first-letter` | — | Supported | The letter's box takes `unicode-bidi: normal` and its parent's `direction`, whatever its style says, as Chrome's cascade gives it. | — |
| Upright text | — | Supported | Text set upright in a vertical line reads left to right, with no bracket pairing. | — |

## Fonts and font selection

winkin selects fonts through **fontwich**, the workspace's font crate. The
host gives the context a `fontwich::Collection`: the system fonts (with
fontwich's `system` feature), application fonts, and web fonts.

### Families and matching

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `font-family` | family names, generic families | Supported | Tried in order. CSS font matching picks the face within a family. Installed families answer to Chrome's aliases: `Times`, `Courier` and `Helvetica` find `Times New Roman`, `Courier New` and `Arial`, and the other way about, with Windows' TrueType stand-ins for its bitmap families. | `default_language` |
| Generic families | `serif`, `sans-serif`, `monospace`, `cursive`, `fantasy`, `math`, `system-ui` | Supported | Each resolves to one family, as Chrome's default font settings for the platform give it. The settings are chosen by the script of the language fonts are chosen in, not the script of the run. | `default_language` |
| Generic families | `ui-serif`, `ui-sans-serif`, `ui-monospace`, `ui-rounded` | Partial | Resolved as `serif`, `sans-serif` and `monospace`. `ui-rounded` resolves as `sans-serif`. Chrome reads none of the `ui-*` families. | — |
| Generic families | `emoji` | Supported | Resolves to the platform's emoji fonts. | — |
| Generic families | `fangsong` | Partial | Resolves to no family, as in Chrome. The text falls back. | — |
| The Standard font | — | Supported | After every `font-family` list, before system fallback, winkin tries the Standard font of the language, as Chrome tries `-webkit-standard`. On Windows that is Times New Roman for most languages and Microsoft YaHei for Simplified Chinese. | `default_language` |
| Initial `font-family` | the empty list | Supported | The empty list is the Standard font alone, as Chrome's initial value is. It is not `serif`. | — |
| `font-weight` | 1 to 1000 | Supported | Matching, then the `wght` axis of a variable font. Bold that the font cannot reach is synthesized, but not in a font whose `wght` axis reaches past 400 unless its `@font-face` rule declares its weights, as in Chrome. | `platform_font_variations` |
| `font-width` (`font-stretch`) | percentages | Supported | Matching, then the `wdth` axis. | `platform_font_variations` |
| `font-style` | `normal`, `italic`, `oblique` with an angle | Supported | Matching, then the `ital` or `slnt` axis, then a synthesized slant. Italic is Chrome's 14deg: an italic request with no italic face takes an oblique one whose range holds 14deg, and drives a `slnt` axis that reaches it whatever the face declares. | `platform_font_variations` |
| `font-size` | a length | Supported | The host computes it in pixels. Sizes above 10,000 px are used at 10,000, as in Chrome. A size that is negative or not a number is zero. | — |
| `lang` | a language tag | Supported | `TextGroup::language`. It chooses generic fonts, fallback, the variant of Han, the OpenType language system, line-breaking rules, hyphenation and case mapping. Where a style has none, fonts and shaping use `Config::default_language`; breaking and case mapping use none, as in Chrome. | `default_language` |

### Fallback

Each grapheme cluster is set in the first font of its list that covers it.
**Coverage comes first**: a character is drawn by some installed font if
any installed font maps it. Choosing the same fallback font as Chrome is a
goal, and fontwich mirrors each platform's Chrome rules to reach it.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Per-cluster fallback | Supported | A cluster that no family in the list maps asks the platform's fallback for its script and language, then the fallback for common characters, then every installed family. A control character stops before every installed family, since the fonts that map one draw it blank, and draws the primary font's missing glyph, as in Chrome. A character no installed font maps is noted once per context and not looked for again. Such a cluster is set in the primary font, which draws its missing glyph. | — |
| Combining sequences | Supported | Coverage allows for the shaper's normalization: a font that covers the composed or decomposed form covers the cluster. | — |
| Continuations | Supported | A cluster split by a style boundary keeps one font for its grapheme, by Chrome's rule. | — |
| Fallback parity | Partial | Chrome's choice of fallback font is matched where fontwich models it. The tables follow Chrome with an English user interface; a host whose users run Chrome in another language supplies that difference itself. | — |
| Line metrics | Supported | Which of a font's vertical metrics size a line: Windows's `OS/2` win metrics, macOS's `hhea`, or Linux's typographic metrics where the font asks for them. A font whose win ascent and descent are both zero is measured by `hhea`, as DirectWrite reports it. | `line_metrics` |

### Features, variations and synthesis

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `font-feature-settings` | tag and value list | Supported | Applied in Chrome's order: `@font-face` descriptor and alternates, capitals, kerning, ligatures, East Asian, numeric, settings, position. | — |
| `font-variation-settings` | axis and value list | Supported | Applied after the matching axes and the `@font-face` descriptor, then `opsz` unless set. Every font takes them, system fonts too. Chrome on Windows and Linux applies them to web fonts only; `Matched` reproduces that. | `platform_font_variations` |
| `font-optical-sizing` | `auto`, `none` | Supported | `auto` sets `opsz` to the font size unless `font-variation-settings` sets it. | `platform_font_variations` |
| `font-kerning` | `auto`, `normal`, `none` | Supported | `none` turns off `kern` and `vkrn`. | — |
| `font-variant-ligatures` | all keywords | Supported | | — |
| `font-variant-numeric` | all keywords | Supported | | — |
| `font-variant-east-asian` | all keywords | Supported | | — |
| `font-variant-alternates` | `normal`, `historical-forms` | Partial | The other values need `@font-feature-values`, which a layout is not given. | — |
| `font-variant-caps` | `normal`, `small-caps`, `all-small-caps`, `petite-caps`, `all-petite-caps`, `unicase`, `titling-caps` | Supported | Where the font lacks the feature and `font-synthesis-small-caps` allows, small capitals are synthesized at 0.7 of the font size, rounded, as Chrome does. Petite capitals and unicase fall back to small capitals as Chrome's table says. | — |
| `font-variant-position` | `normal`, `sub`, `super` | Supported | By default a run the font's `subs` or `sups` does not cover whole is synthesized whole, sized and raised by the font's own `OS/2` metrics, as CSS Fonts 4 asks. The line does not grow for it. Chrome applies the feature only and synthesizes nothing (a Chrome bug); `FeaturesOnly` reproduces that. | `position_synthesis` |
| `font-variant-emoji` | `normal`, `text`, `emoji`, `unicode` | Supported | Chooses text or emoji presentation where the text has no variation selector, as Chrome does. | — |
| `font-synthesis` | `weight`, `style`, `small-caps`, `position` | Supported | Faux bold widens nothing, as in Chrome. The host draws the embolden and skew it reads from each run's font. | — |
| `font-size-adjust` | `none`; a number with `ex-height`, `cap-height`, `ch-width`, `ic-width` or `ic-height` | Supported | One used size per style from the primary font's metric. A missing metric counts as one em, as in Chrome. Sizes are floored to 1/100 px. At `0` the text is set at size zero, as in Chrome. | — |
| `font-size-adjust` | `from-font` | Not supported | No value in the API. | — |
| `font-language-override` | `normal`, a tag | Supported | Sets the OpenType language system, as written: `trk` is not the Turkish system `TRK`. | — |
| Letter-spacing and ligatures | — | Supported | Under a nonzero `letter-spacing`, `liga`, `clig` and `calt` are off and `dlig` and `hlig` are not applied, as in Chrome. | — |
| `text-spacing-trim` and `chws` | — | Supported | `chws` is on by default where `text-spacing-trim` trims, unless the settings name it or turn on `halt` or `palt`, as in Chrome. | — |

### Emoji and variation sequences

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Emoji presentation | Supported | VS15 and VS16, keycaps, flags, ZWJ sequences and `font-variant-emoji` choose a text or an emoji font as Chrome does. | — |
| Variation sequences | Supported | A cluster with a selector takes the first font in the whole list, fallback included, that maps the sequence; else the first that maps the base, with the selector hidden. Any sequence a font's `cmap` lists counts, where Chrome counts only Unicode's standardized ones. VS15 and VS16 choose presentation instead. Fallback is asked for the sequence, where Chrome asks for the base. | — |

### `@font-face` and web fonts

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| `unicode-range` | Supported | Through fontwich's faces. | — |
| Pending loads | Supported | `Layout::wanted_faces` lists the faces still downloading that the content wants. Until one lands, the text is set in whatever covers it. | — |
| Descriptors | Supported | `font-feature-settings`, `font-variation-settings`, `size-adjust`, `ascent-override`, `descent-override`, `line-gap-override`, as Chrome applies them. `size-adjust` does not apply under `font-size-adjust`. | — |

## Spacing

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `letter-spacing` | a length | Supported | Not applied in cursive scripts such as Arabic, as in Chrome. It goes once after each cluster as shaped: where the font joins two graphemes, as a Bengali ya-phala does, the pair takes it once, as in Chrome. A percentage is of each element's own font size; the host resolves it per element. | — |
| `word-spacing` | a length and a percentage | Supported | The percentage is of the style's own font size, as in CSS Text 4 and Chrome. By default it widens U+0020 and U+00A0 only, and not a U+0020 at the text's start unless the block keeps its spaces, as Chrome does. `Css` widens every word separator CSS lists, wherever it is. | `word_spacing` |
| `text-autospace` | `no-autospace`, `normal`, `ideograph-alpha`, `ideograph-numeric` | Supported | The initial value is `no-autospace`, as Chrome computes it. The space is an eighth of the primary font's `ic`, and works across element boundaries. After right-to-left text it is drawn before the later text, as in Chrome. In the sideways modes text is horizontal, so upright text there ends no space. | — |
| `text-autospace` | `punctuation`, `insert`, `replace`, `auto` | Not supported | No value in the API. | — |
| `text-spacing-trim` | `normal`, `space-all`, `space-first`, `trim-start` | Supported | Trimming goes through the font's `halt`, as in Chrome. In a font without `halt` Chrome trims nothing; `Always` trims by halving the mark's own advance. A neighbour is classed in the font it is shaped in, so a pair set in two fallback fonts trims as in Chrome. A trimmed pair broken across lines is reshaped. | `punctuation_trim` |
| `text-spacing-trim` | `trim-both`, `trim-all`, `auto` | Not supported | No value in the API. | — |
| `hanging-punctuation` | `first`, `last`, `allow-end`, `force-end` | Beyond Chrome | Chrome parses it but lays out nothing by it. The marks are CSS Text 4's: `first` hangs Unicode's Ps, Pi and Pf, the straight quotes and U+3000; `last` Pe, Pi and Pf and the straight quotes. A hanging mark is left out of both intrinsic sizes, and one under `allow-end`, which hangs only where it must, out of min-content alone. | — |
| `line-padding` | a length | Beyond Chrome | Chrome does not parse it. | — |

## Alignment and justification

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-align` | `start`, `end`, `left`, `right`, `center`, `justify` | Supported | Per paragraph direction. The host resolves `match-parent`, and maps `justify-all` to `justify` with `text-align-last: justify`. | — |
| `text-align-last` | `auto`, `start`, `end`, `left`, `right`, `center`, `justify` | Supported | `auto` is `start` under `justify`, else `text-align`. | — |
| `text-justify` | `auto`, `none`, `inter-word`, `inter-character` | Supported | Opportunities are counted as Chrome counts them: spaces, U+00A0, tabs and the gaps between CJK characters. The room is divided as Chrome divides it. The host maps `distribute` to `inter-character`. | `tab_justification` |
| Tabs in justified lines | — | Supported | Chrome stretches a tab as it stretches a space, so tab stops drift. `KeepStops` keeps each tab at its stop. | `tab_justification` |
| `text-group-align` | `none`, `start`, `end`, `left`, `right`, `center` | Beyond Chrome | Chrome does not parse it. | — |

## Line height and vertical alignment

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `line-height` | `normal`, a number, a length | Supported | The host resolves a percentage to a length. `normal` takes the metrics of the fallback fonts a line uses, as Chrome does. Half-leading is floored. A line keeps the strut of every box open across its start, as in Chrome. | `line_metrics` |
| `vertical-align` | `baseline`, `sub`, `super`, `text-top`, `text-bottom`, `middle`, `top`, `bottom`, a length, a percentage | Supported | A percentage is of the element's own line height. `top` and `bottom` are settled per line. Chrome moves a `top` box with the shifts of the box it waits for, which CSS 2.1 does not ask; winkin does not copy it. Not inherited, but shifts add up down the tree. | `super_sub` |
| `vertical-align: sub`, `super` | — | Supported | By default a share of the parent's font size: up a third plus one pixel, down a fifth plus one pixel, as in Chrome. `FontMetrics` uses the parent font's `OS/2` offsets. | `super_sub` |
| `dominant-baseline` | `auto`, `alphabetic`, `ideographic`, `central`, `mathematical` | Beyond Chrome (opt-in) | Chrome reads it only in SVG. By default winkin ignores it too. `Applied` sets each box's dominant baseline on its parent's, for CJK text on the ideographic baseline among Latin. | `dominant_baseline` |

## Inline boxes

A box is opened with `LayoutBuilder::open_box` and closed with `close_box`.

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `margin`, `border-width`, `padding` | lengths per physical side | Supported | The host resolves percentages against the block's width and snaps border widths as CSS does (below 1 px to 1, else down to a whole pixel). Each length is truncated onto the 1/64 px grid, as Chrome holds it. The sides along the line take room on it; the sides across it do not. | — |
| `box-decoration-break` | `slice`, `clone` | Supported | Under `clone` every fragment takes both edges, and a line holds a cloned box's closing edge. Chrome 153 ships letting that edge overflow, a bug fixed behind a flag; winkin has the fix. | — |
| Culling | — | Supported | A box with no edges, no `vertical-align`, nothing it paints and its parent's font metrics keeps no fragment, as Chrome culls one. Set `ComputedStyle::paints` for a box with a background, border or outline. `Layout::box_fragments` still answers for a culled box, from its descendants. | — |
| Text decoration | — | Supported | Set `ComputedStyle::decorates` for a box whose text is underlined, overlined or struck through. `Line::paints` hands out each decoration bar with its baseline and the font's underline metrics; the host draws it. For `auto` a bar gives Chrome's thickness and gap (`underline_thickness`, `underline_gap`). For `text-decoration-thickness: from-font` and `text-underline-position: from-font` it gives the font's `post` underline, falling back to `auto` as Chrome does (`underline_thickness_from_font`, `underline_gap_from_font`), and the font's own lines as they are (`font_underline`, and `font_line_through` from `OS/2`, which Chrome never reads). | — |
| White space hanging in a box | — | Supported | A box covers the white space that hangs inside it, as Chrome draws it. | — |
| `text-box-trim` on an inline box | — | Not supported | Accepted; it keeps the box from being culled and trims nothing. | — |
| `line-fit-edge` | — | Not supported | Accepted and ignored. | — |

## Atomic inlines

An inline-block or a replaced element is pushed with
`LayoutBuilder::atomic`, with the border box size the host laid it out at
(`BoxSize`) and its style for margins and alignment.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Size and baseline | Supported | `BoxSize::baseline` is `None` for a box with no baseline, which aligns its margin box's bottom edge. Sizes and margins are truncated onto the grid, as in Chrome. `Layout::set_atomic_sizes` changes block sizes and baselines after building, outside ruby and initial letters. | — |
| `vertical-align` | Supported | Against the margin box. An atomic has no strut of its own, as in Chrome. | — |
| In the text | Supported | One U+FFFC: it has an offset, a caret on either side and a bidi class. It ends a run of white space. | — |

## text-indent

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-indent` | a length and a percentage, `hanging`, `each-line` | Supported | A percentage is of the line's width, the `Area`'s inline size. Works beside floats and in intrinsic sizes. | — |

## Floats

A float is pushed with `LayoutBuilder::float` where it is anchored in the
text, with its border box size and its style for margins. The host lays out
its contents first.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Placing | Supported | When the text reaches a float, the breaker hands the host a `FloatRequest` through the `Exclusions` trait: the margin box size and where it may go. The float goes on the current line if it fits there, else after it, by Chrome's rule. The host places it and says where. | — |
| Bands | Supported | The host's `Exclusions::band` says how much room each line has. A line that does not fit moves down to where the band changes. | — |
| Trial breaks | Supported | Balancing, `pretty` and refitting take back floats they placed through `checkpoint` and `rewind`, as Chrome does. | — |
| `clear`, `shape-outside`, floats from other blocks | Supported | The host's: it answers bands with them. | — |
| Floats past a line clamp | Supported | Not placed. | — |

## Absolutely positioned boxes

An absolutely positioned box in inline content, `position: absolute` or
`fixed`, is pushed with `LayoutBuilder::absolute` where it is in the text,
with its outer display before positioning blockified it
(`OriginalDisplay`). It goes in as an anchor that takes no room, as Chrome's
out-of-flow item does. After line breaking, `Layout::static_positions` says
where each box's static position is. The host lays the box out and places
it.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Text processing | Supported | The anchor adds no text. White space collapses across it, capitalize reads past it, and it changes no break opportunity. Bidi resolves it as a U+FFFC. It splits shaping, as Chrome's `ShapeText` does, so no kerning or ligature crosses it; CSS would have it ignored. | — |
| The line it falls on | Supported | Inside a line it stays where it is. At a soft break it ends the line where Chrome's breaker breaks after it: ahead of any box opening at the break, where the line's white space before it fits, and not under a `balance` or `pretty` plan. Otherwise it starts the next line, with a box opening around it. Before a forced break it ends the line; after one it starts the next. | — |
| Inline-level static position | Supported | Where the anchor would have been on its line, after alignment, justification and bidi reordering, at the line box's top. Its direction is its bidi level's, as in Chrome: right to left, its right edge goes there. | — |
| Block-level static position | Supported | At the block's start edge, whatever the floats and `text-indent`, at the line box's top, or at its bottom where in-flow content comes before it on the line, as in Chrome. Its direction is the block's. | — |
| An empty line | Supported | An anchor no line box holds, after a final forced break or in content with no line, stands where an empty line after the last would start: aligned, and indented where a first line would be. | — |
| Ruby annotations | Partial | An anchor inside an annotation stands on its base's line, where its place in the base's text is. | — |
| Under a clamp | Supported | An anchor past a clamp's last line stands at the block's end. | — |

## ::first-letter and initial-letter

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `::first-letter` | — | Supported | `LayoutBuilder::set_first_letter` before the text. The letter is found as CSS Pseudo-Elements 4 finds it and Chrome's `FirstLetterLength` counts it: white space that collapses away before it is skipped, kept white space before it (a tab, say) is taken, punctuation before and after is taken, and the letter is one grapheme cluster. The letter takes `unicode-bidi: normal` and its parent's `direction`. Its runs carry the pseudo-element's key. | — |
| `initial-letter` | size and sink | Supported | On a `::first-letter` box or the block's first inline box. Sized as Chrome sizes it, in every writing mode. The box fits its ink and is kerned, sunk or raised as Chrome does. A tab in it takes its advance. As Chrome's atomic letter box, it stands at the line's start in either direction, takes a second `text-indent` inside it, adds nothing to the room ruby makes over its line but moves down with that line, and the decorations of the boxes around it skip it. The host receives its exclusion as a `FloatRequest`, keyed by the box, on the paragraph's start side. Placing it as Chrome does (at its block start, even above earlier floats; later floats on its side and blocks that open with an initial letter clear it) is the host's. | — |
| `initial-letter-align` | `alphabetic`, `hanging`, `ideographic` | Beyond Chrome | Chrome parses only `alphabetic`. The others follow CSS Inline 3. | — |
| Ink bounds | Partial | The letter fits the glyph outline as designed. Chrome on Windows fits DirectWrite's hinted bounds, about a pixel wider on each side. A host that supplies hinted metrics (`LayoutBuilder::finish_with_metrics`) can match it. | — |

## ::first-line

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `::first-line` | — | Supported | Give `ComputedBlockStyle::first_line` and a first-line style for each box. The first line may change the whole font group, with its families, features and variations, `text-transform`, `letter-spacing`, `word-spacing`, `line-height`, `vertical-align` and emphasis. Anything else it sets is taken from the element's own style. Works beside floats, balanced and in vertical text. | — |
| First-line `text-transform` | — | Supported | Replaces the element's transform, as the cascade gives it and as Firefox does. Chrome composes the two; winkin does not. | — |

## text-overflow and line-clamp

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-overflow` | `clip`, `ellipsis` | Supported | A line that overflows is cut from its end, in visual order, as Chrome cuts it. A box is never cut. The ellipsis is U+2026, or three full stops where the block's fonts lack it. The tail is laid out and marked hidden, not dropped. The room is measured in the band where the line is placed. | `ellipsis_space` |
| `text-overflow` | a string, two values | Not supported | | — |
| Space before an ellipsis | — | Supported | A space kept before the cut stays before the ellipsis, as in Chrome. `Hidden` moves it into the hidden tail, so the ellipsis follows the last word. | `ellipsis_space` |
| `line-clamp` | `none`, a number | Beyond Chrome | CSS Overflow 4's `line-clamp`. Chrome ships only `-webkit-line-clamp` on a `-webkit-box`; the host may map that here. The last line kept is cut for an ellipsis where text is left after it. Nothing after it is broken; Chrome lays the rest out clipped. A clamp the text does not reach clamps nothing. | — |
| `line-clamp` | `auto` | Beyond Chrome | Keeps the lines that end within `Area::block_end`, which the host sets from the block's `height` or `max-height`. Chrome has it behind a flag. | — |
| `block-ellipsis` | `auto` only | Partial | Every clamp is cut as `text-overflow` cuts. No `no-ellipsis`, no string, and no moving the end to a soft wrap opportunity. | — |
| Clamp point between blocks | — | Not supported | A clamp ends one block's lines only. | — |

## text-box-trim and text-box-edge

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-box-trim` (on the block) | `none`, `trim-start`, `trim-end`, `trim-both` | Supported | Trims the first line's over edge and the last line's under edge. The amounts are in `LayoutMetrics::trim_start` and `trim_end`. Negative amounts work as in Chrome. In `vertical-lr` the start is the under side, as in Chrome. | — |
| `text-box-edge` | over: `text`, `cap`, `ex`; under: `text`, `alphabetic` | Supported | As Chrome. A vertical line's edges are taken from its central baseline. | — |
| `text-box-edge` | `ideographic` | Beyond Chrome | The ideographic em box. | — |
| `text-box-edge` | `ideographic-ink` | Partial | Accepted and trimmed as `ideographic`. | — |
| `text-box-trim` on inline boxes, `line-fit-edge` | — | Not supported | See [Inline boxes](#inline-boxes). | — |

## text-wrap: balance and pretty

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-wrap-style` | `auto`, `stable` | Supported | Greedy. `stable` lays out as `auto`. | — |
| `text-wrap-style` | `balance` | Supported, beyond Chrome by default | Chrome's scorer, as Chrome balances a paragraph of six lines or fewer. Chrome balances only the first paragraph and gives up past six lines or beside floats; winkin balances every paragraph, longer ones and beside floats, by halving the room. An initial letter's lines are balanced too. A clamp's kept lines are balanced. A width whose lines need an `overflow-wrap` emergency break is not taken, as Chrome stops balancing after an overflow; `pretty` treats such a line as overflowing. | — |
| `text-wrap-style` | `pretty` | Supported | By default Chrome's rule: it acts where the last line is one word under a third of the line, or the two lines before it end in hyphens, over the last four lines. `Even` also acts on any last line under a fifth, over the last six lines. | `pretty` |

## Ruby

Ruby is pushed with `LayoutBuilder::open_ruby`, `open_annotation`, `close_annotation`
and `close_ruby`. Base text follows the container; base text after an
annotation starts the next column. Annotations are read back with
`Line::annotations` and painted as `Paint::Annotation`.

| Property or feature | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| Pairing | — | Supported | Bases pair with annotations in order. | — |
| Ruby container box | — | Supported | The container is an inline box: its margin, border and padding take room at both ends, with or without annotations, and it keeps a fragment, which `Layout::box_fragments` finds, as Chrome keeps one. | — |
| Bidi in ruby | `direction`, `unicode-bidi` | Partial | The container's own controls apply to its base, and an annotation's inside its isolate, so `dir` on either orders its text, as in Chrome. A column's box brackets its base however it reorders. Chrome also isolates each column in the container's direction and resolves an annotation in its column's isolate: winkin sets an annotation as a first-strong isolate, and a container isolated against its paragraph keeps its columns in logical order. | — |
| Annotations outside ruby | — | Supported | `LayoutBuilder::open_annotation` outside every ruby container opens an anonymous one over an empty base, as Chrome wraps a `ruby-text` box whose parent is no ruby. The annotation keeps its own style and edges; `close_annotation` closes both. | — |
| Empty bases | — | Supported | A base with no text, or holding only a float, keeps its annotations over the ruby container's em box, and nothing overhangs beside it, as in Chrome. | — |
| `<rtc>` levels | — | Beyond Chrome | Each annotation after the first in a column is the next level, stacked outward. `LayoutBuilder::open_annotation_with_position` opens an `<rt>` inside an `<rtc>`, on the side of the `<rtc>`'s `ruby-position`. Chrome gives `rtc` no box and sets its text as base text. | — |
| Nested ruby | — | Supported | A ruby inside another's base is its own column, one unit of the outer base. The outer annotation spans the whole nested base, and the inner annotations sit nearer the base, as Chrome stacks them. A ruby inside an annotation, or more than 32 deep, becomes a box whose annotations are the enclosing column's next levels. | — |
| `ruby-position` | `over`, `under` | Supported | Initially `over`, as Chrome computes it. Each annotation goes on its own container's side. | — |
| `ruby-position` | `alternate`, `alternate under` | Beyond Chrome | Chrome lacks `alternate`. Levels alternate sides. | — |
| `ruby-align` | `space-around`, `space-between`, `center`, `start` | Supported | For bases and annotations, as Chrome applies it. | — |
| `ruby-overhang` | `auto`, `none`, `spaces` | Supported | By default an annotation reaches as far as Chrome lets it. Under `spaces` it reaches over spaces and tabs only, and never over a blank another column already reaches over. `KanaOnly` lets it reach over kana only, at most one annotation em, as the Japanese Layout Requirements ask. | `ruby_overhang` |
| Line breaking around ruby | — | Supported | A line may break after a column by the base's end and what follows, as in Chrome. A column that starts a line keeps its annotation there. | — |
| Breaking inside a ruby base | — | Partial | A column that does not fit breaks inside its base, as Chrome breaks it: each annotation is cut in proportion, and continues on the next line. A short remainder (four base glyphs and eight per annotation, Chrome's rule) stays whole. Min-content sizes use the pieces. `text-wrap-style: pretty`, `balance` and `nowrap` still move or overflow a column whole. By default an annotation with no break opportunity goes whole with the first piece, as in Chrome. `AllLevels` breaks a base only where every non-empty annotation can break too, as CSS Ruby asks. | `ruby_break_within` |
| Justified lines | — | Supported | A base is justified with its line where no annotation is wider, else as one object. A column at a justified line's edge is flush to the edge. | — |
| Shifted bases | — | Supported | An annotation over a raised or lowered base stands on the shifted text, as in Chrome. | — |
| Boxes inside annotations | — | Supported | Their edges take room on the annotation line, and they get box fragments of their own. An annotation's own key finds its box too: its annotation line, as wide as its column, as Chrome's client rect for an `<rt>` is. | — |
| Atomic inlines inside annotations | — | Supported | Each takes its margin box on the annotation line, as on a line. `Annotation::atomics` reads them, and `Line::paints` paints them with the annotation's text. | — |
| Room for annotations | — | Supported | Lines grow for their annotations. The first line may use `Area::room_above`, the room the block before lends. `Layout::room_below` is the room the last line leaves for the next block; after a clearing `<br>`, only what reaches past the clearance counts. | — |
| `<br>` and segment breaks in ruby | — | Supported | Each is a space, as in Chrome, and a clearing `<br>` there clears nothing. One beside a ruby clears as anywhere. | — |
| `ruby-merge` | — | Not supported | | — |

## Emphasis marks

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `text-emphasis-style` | any | Supported | Layout reads only whether marks are set (`TextEmphasis::marks`). The host draws the mark's shape. `TextRun::emphasis_marks` gives each mark's middle, baseline and size (half the text's size, rounded to a pixel), one per grapheme cluster that takes one. The host centres the mark glyph's ink bounds on the middle, in vertical text too, as Chrome does. | — |
| `text-emphasis-position` | `over`, `under`; `right`, `left` | Supported | `over` and `under` apply in horizontal and sideways lines, `right` and `left` in vertical ones, as CSS Text Decoration 4 says. Chrome reads `right` and `left` in sideways lines too; winkin does not copy that. Marks go past an annotation on the same side. | — |
| `text-emphasis-skip` | `spaces`, `punctuation`, `symbols`, `narrow` | Beyond Chrome | Chrome always skips spaces and punctuation, the initial value. | — |
| `text-emphasis-color` | — | Supported | The host's: it paints the marks. | — |
| Room for marks | — | Supported | By default a line takes what its marks need beyond the room the line before left, as Chrome does, so lines can space unevenly. `Uniform` grows every marked line alike. | `emphasis_room` |
| Combined text | — | Supported | Takes one mark in the middle of its em. | — |

## Vertical writing

Positions are line-relative in every writing mode: along the line from the
line box's left, across it from its top. The host maps them onto the page.

| Property | Values | Status | Notes and caveats | Config |
|---|---|---|---|---|
| `writing-mode` | `horizontal-tb`, `vertical-rl`, `vertical-lr`, `sideways-rl`, `sideways-lr` | Supported | A vertical line is centred on its central baseline, as Chrome rounds it. The sideways modes set horizontal lines turned whole, on the alphabetic baseline. | — |
| `text-orientation` | `mixed` | Supported | By each character's Vertical_Orientation: U, Tu and Tr upright, as in Chrome. A Tr character's vertical form is the font's `vert`. A grapheme stands as its first character. | — |
| `text-orientation` | `upright` | Supported | Follows CSS Writing Modes 4, where Chrome stands everything upright. Every cluster upright, except vertical-only scripts (Mongolian, Phags-pa), which keep their intrinsic sideways orientation. A Common or Inherited character (a space, punctuation, a digit) stands as the script run it resolves into: on its side between Mongolian words, upright between Latin ones. | — |
| `text-orientation` | `sideways` | Supported | | — |
| Vertical metrics | — | Supported | Upright text is shaped down the line with the font's `vmtx` and `VORG`, or Chrome's fallbacks. | — |
| `text-combine-upright` | `none`, `all` | Supported | One unit, one em, no break inside. It is fitted with `hwid`, `twid`, `qwid`, then narrowed, as Chrome does. White space at its start and end collapses on its own, as in Chrome, which sets it as an inline block. No spacing, one ideograph's justification and one emphasis mark. | — |
| `text-combine-upright` | `digits` *n* | Beyond Chrome | Chrome does not parse it. *n* is taken from 2 to 4. | — |
| Hyphens and ellipses | — | Supported | Stand as the text they end stands: upright under `upright`, and by their characters' orientation under `mixed`, which sets a hyphen on its side, as in Chrome. | — |
| Decorations | — | Supported | Offsets from the line's dominant baseline. The host draws them. | — |

## Selection and hit testing

Not CSS, but an integration needs them. Everything is in
`winkin::selection` and on `Layout`. Positions are an offset into
`Layout::text` and an affinity, and survive relayout unchanged.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Hit testing | Supported | `Layout::hit_test`. A point past the lines hits its column on Windows and the line's start or end on macOS and Linux. Ellipses and hyphens pass through to the text under them. Annotations can be hit. | per call |
| Carets | Supported | `Layout::caret` gives the caret Chrome draws. `Layout::carets` adds the second caret at a change of direction, for a host that draws a split caret (beyond Chrome). Works in vertical lines and in combined text. | — |
| Motion | Supported | `Selection::modify` with a `Motion`: by character, word, line (keeping the column), line start or end, paragraph and text. Word motion stops after the spaces on Windows and at the word's end elsewhere (`WordMotion`, per motion). `Left` and `Right` move the way they point on the screen; a host that moves as Chrome does maps arrows to `Forward` and `Backward` by `Layout::paragraph_direction`. | per motion |
| Selection rectangles | Supported | `Layout::selection_rects`, as tall as the line box, as Chrome paints them. | — |
| Copying | Supported | `Layout::selected_text`, as `Selection.toString()` gives it, or with `CopyKind::Clipboard` with no-break spaces as spaces, as Chrome's clipboard has it. | per call |
| The host's offsets | Supported | `Layout::node_position` and `Layout::position` convert to and from a node and an offset in the text the host gave it. Build with `BuildOptions::map_source` set. | — |

## Text on paths

`winkin::path` sets a layout's lines along curves, as SVG's `textPath`
does. Lines are broken and laid out as usual; only where they are drawn
changes.

| Feature | Status | Notes and caveats | Config |
|---|---|---|---|
| Paths | Supported | `PathRoom` gives each line the length of its own path. `Polyline` and `BezierPath` are provided; a host can implement `TextPath` over its own curve library. | — |
| Glyph placement | Supported | Each character turns to the path where its middle falls, as SVG places it. Upright and combined text stands across the path. | — |
| What follows the path | Supported | Hyphens, ellipses, annotations, emphasis marks, atomics and decorations (one piece per character). Box backgrounds and borders are not drawn on a path, as in SVG and Chrome. | — |
| Past the ends | Supported | Hidden by default, as SVG and Chrome do. `PastEnds::Straight` runs on straight past each end. | per call |
| SVG attributes | Partial | `side="right"` is `ReversedPath`. `startOffset` is where `PathRoom` starts each line. `text-anchor` is the line's `text-align`. `method="stretch"` is the renderer's, through `TextPath::point`. Closed paths do not wrap round, as in every browser. | — |

---

## Not supported

These have no value in the API, or are accepted and ignored.

- `hyphens: auto`: accepted and treated as `manual`. There is no dictionary
  hyphenation. `hyphenate-limit-*` is not read.
- `word-break: auto-phrase`.
- Breaking inside a ruby base under `text-wrap-style: pretty` or
  `balance`: the column moves or overflows whole.
- `ruby-merge`.
- `text-box-trim` on inline boxes, and `line-fit-edge`.
- `text-box-edge: ideographic-ink` as its own edge: trimmed as
  `ideographic`.
- `block-ellipsis` beyond `auto`: no `no-ellipsis`, no string, no moving
  the cut to a soft wrap opportunity.
- A `line-clamp` point between blocks.
- `text-overflow` with a string or two values.
- `text-fit` (`text-grow`, `text-shrink`).
- `font-size-adjust: from-font`.
- `font-variant-alternates` other than `historical-forms` (they need
  `@font-feature-values`).
- `text-autospace` values `punctuation`, `insert`, `replace` and `auto`.
- `text-spacing-trim` values `trim-both`, `trim-all` and `auto`.
- Outside list markers. A host can push an inside marker as text or an
  atomic inline.
- Block layout itself: one layout is one block's inline content. See
  below.

## What the host does

winkin lays out lines. The host does everything around them.

- **The cascade.** Compute every value: inheritance, `em` and percentages
  against the containing block, `match-parent`, `line-break: auto`,
  shorthands such as `white-space` and `-webkit-line-clamp`, `letter-spacing`
  percentages per element, and border widths snapped to whole pixels. Hand
  in a `ComputedStyle` for each box and a `ComputedBlockStyle` for the block,
  with its `::first-line` styles where it has them.
- **Font loading.** Build a `fontwich::Collection`: system fonts (enable
  fontwich's `system` feature), application fonts, and `@font-face` faces.
  Fetch the faces in `Layout::wanted_faces`, add each when it lands, give
  the context the new collection with `Context::set_collection`, and
  rebuild the layouts that wanted it.
- **Painting.** Draw everything `Line::paints` hands out, in its order:
  backgrounds, borders, text runs (glyph ids, positions and fonts),
  decoration lines, emphasis marks, hyphens and ellipses, atomic inlines.
  Colours, decoration styles, mark shapes and shadows stay with the host,
  looked up by each item's `NodeKey`. Synthesized bold and oblique are the
  renderer's to draw.
- **Atomic inlines and floats.** Lay out their contents first, and hand in
  their border box sizes and baselines.
- **Absolutely positioned boxes.** Lay each out and place it in its
  containing block. Where an inset is `auto`, start from its static
  position in `Layout::static_positions`, mapped onto the page as the lines
  are.
- **Floats and exclusions.** Implement `Exclusions` for the enclosing block
  formatting context: place each float the breaker hands over, answer each
  line's band, and handle `clear` and `shape-outside`. `NoExclusions` serves
  a block with no floats.
- **Block layout.** Stack blocks, collapse margins, and place each layout's
  lines with the `Area` it is broken in. Read `Layout::metrics` for the
  block's end, baselines and trims, and `Layout::intrinsic_sizes` for
  min-content and max-content. Lend `Layout::room_below` to the next block
  as its `Area::room_above` where annotations or marks need it.
- **Writing modes.** Map line-relative positions onto the page.
- **Hinted metrics (optional).** Supply glyph advances and bounds from the
  renderer's rasterizer through `font::FontMetricsProvider`, with
  `LayoutBuilder::finish_with_metrics` and
  `Layout::break_lines_with_metrics`. Without it, winkin reads unhinted
  metrics from the font.
- **Limits.** A build never fails or panics. Text past 2^30 − 1 bytes, or
  more than 65,535 distinct styles or used fonts, is dropped or replaced,
  and `BuildReport` says how much.

## Config at a glance

Every field of `winkin::config::Config`. All three Chrome presets share
every value except `line_metrics`. The Spec column is
`Config::spec()`: CSS's choice where it differs from Chrome, and a portable
value where CSS leaves the choice open (`line_metrics`, `default_language`,
`pretty`, `ellipsis_space`, `ruby_overhang`).
`Config::default()` and `Config::platform()` are the preset for the build
target: Windows on Windows, macOS on Apple platforms, Linux everywhere else
(wasm and embedded included).

| Field | Windows | macOS | Linux | Spec | Other value | What it chooses |
|---|---|---|---|---|---|---|
| `line_metrics` | `Win` | `Hhea` | `TypoOrHhea` | `TypoOrHhea` | — | Which font metrics size a line: `OS/2` win metrics (DirectWrite), `hhea` (Core Text), or typographic metrics where the font sets `USE_TYPO_METRICS`, else `hhea` (FreeType). |
| `default_language` | `en` | `en` | `en` | `und` | any tag; `und` | The language fonts and shaping use where a style names none. `und` lets each run choose by its own script (beyond Chrome). |
| `super_sub` | `SizeRatio` | `SizeRatio` | `SizeRatio` | `FontMetrics` | `FontMetrics` | How far `vertical-align: super` and `sub` move: Chrome's share of the font size, or the font's `OS/2` offsets. |
| `dominant_baseline` | `Ignored` | `Ignored` | `Ignored` | `Applied` | `Applied` | Whether `dominant-baseline` moves inline boxes. `Applied` is beyond Chrome. |
| `position_synthesis` | `SynthesizeMissing` | `SynthesizeMissing` | `SynthesizeMissing` | `SynthesizeMissing` | `FeaturesOnly` | Whether `font-variant-position` synthesizes what the font lacks, as CSS asks. `FeaturesOnly` is Chrome's. |
| `platform_font_variations` | `All` | `All` | `All` | `All` | `MatchingOnly` | Whether system fonts take `font-variation-settings` and `opsz`. `MatchingOnly` is Chrome's on Windows and Linux. |
| `small_kana` | `MayStartLine` | `MayStartLine` | `MayStartLine` | `Held` | `Held` | Whether a small kana may start a line under `line-break: normal`. `Held` is beyond Chrome. |
| `punctuation_trim` | `FontFeature` | `FontFeature` | `FontFeature` | `Always` | `Always` | Whether `text-spacing-trim` trims in fonts without `halt`. `Always` is beyond Chrome. |
| `pretty` | `Limited` | `Limited` | `Limited` | `Limited` | `Even` | The rule `text-wrap-style: pretty` follows. `Even` is beyond Chrome. |
| `tab_justification` | `Stretch` | `Stretch` | `Stretch` | `KeepStops` | `KeepStops` | Whether justification stretches tabs. `KeepStops` is beyond Chrome. |
| `ellipsis_space` | `Kept` | `Kept` | `Kept` | `Kept` | `Hidden` | Whether a space before an ellipsis cut stays before the ellipsis. `Hidden` is beyond Chrome. |
| `word_spacing` | `SpaceAndNoBreakSpace` | `SpaceAndNoBreakSpace` | `SpaceAndNoBreakSpace` | `WordSeparators` | `WordSeparators` | Which characters `word-spacing` widens. `WordSeparators` is beyond Chrome. |
| `ruby_overhang` | `AdjacentText` | `AdjacentText` | `AdjacentText` | `AdjacentText` | `KanaOnly` | How far a wide annotation reaches over its neighbours. `KanaOnly` is beyond Chrome. |
| `ruby_break_within` | `BaseOpportunities` | `BaseOpportunities` | `BaseOpportunities` | `AllLevels` | `AllLevels` | Whether a ruby base may break where an annotation cannot. `AllLevels` requires every non-empty annotation to break too. |
| `emphasis_room` | `Shared` | `Shared` | `Shared` | `Uniform` | `Uniform` | How lines make room for emphasis marks. `Uniform` is beyond Chrome. |

The font stage, shaping and measuring read their fields when a layout is
built; the breaker and line layout read theirs at each `break_lines`.

Some choices are made per call, not in `Config`:

| Choice | Where | Default |
|---|---|---|
| What a point past the lines hits | `Layout::hit_test` (`PastLines`) | Explicit argument; `PastLines::platform()` selects `Column` on Windows and `LineEnds` elsewhere |
| Where a word motion stops | `Motion::with_word_motion` (`WordMotion`) | `WordMotion::PLATFORM`: `SkipSpaces` on Windows, `StopAtWordEnd` elsewhere |
| What a copy makes of U+00A0 | `Layout::selected_text` (`CopyKind`) | `Text`, keeping it |
| Text past a path's ends | `path::paints` (`PastEnds`) | `Hidden` |
| Dictionaries for Southeast Asian and CJK words | Cargo feature `dictionaries` | on |

## How this is tested

winkin is held to Chrome by ratchets: any change in a result, better or
worse, fails the tests until it is recorded.

- **WPT's inline-text suites.** Run through Blitz's WPT runner, winkin
  passes 3,482 of the 5,078 tests in css-text, css-writing-modes,
  css-fonts, css-text-decor, css-inline, css-ruby and CSS 2's text and line
  box suites. Chrome 153 passes 4,396 of them.
- **Web platform tests.** 137 reftest pairs from css-text, css-ruby,
  css-inline and css-break, compared as the pictures they
  paint. 128 pass, 6 fail and 3 cannot be compared. The failures are an
  annotation paired with one `<rb>`, a block inside `rt`, the two
  `text-fit` tests, and two `line-clamp: auto` tests that need a clamp point
  between blocks.
- **Probe pages.** 112 pages measured in Chrome 153 on Windows with system
  fonts, row by row. 99 agree, and 13 measure themselves and are excluded.
  Where winkin deliberately differs from Chrome, the page agrees with that
  difference recorded.
- **Chrome's numbers.** 197 figures measured in Chrome, each pinned.
- **Conformance.** Unicode's BidiTest, BidiCharacterTest and
  GraphemeBreakTest, and the Unicode property tables swept against ICU.
