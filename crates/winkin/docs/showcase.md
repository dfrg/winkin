# Showcase

Each image mixes several CSS features in one specimen. winkin laid out the
text in the Blitz browser engine with fonts that ship with Windows 11
(Palatino Linotype, Yu Gothic and Consolas), and Blitz painted it. The
images are 560 CSS px wide at a device pixel ratio of 2. The grey tag above
each specimen names its features.

## First line and initial letter

A capital sunk three lines deep opens a justified paragraph whose first line
is set in small capitals. The letter takes its room from the lines beside
it.

![A paragraph opening with a red capital T three lines deep, its first line in blue small capitals, justified.](showcase/first-line-initial-letter.png)

## Inline boxes across lines

Rounded, padded and bordered inline boxes break across lines. Under
`box-decoration-break: slice` the box is cut open where it wraps; under
`clone` each line's part has its own padding, border and rounded corners.
The lower pair nests one box inside another.

![Two columns of the same text: on the left, inline boxes sliced open at each line break; on the right, each line's part of the box closed with its own border and corners.](showcase/box-decoration-break.png)

## Ruby and emphasis marks

Japanese text carries ruby readings and sesame emphasis marks over the same
lines, which make room above for both. Below, emphasis dots sit over a base
whose ruby is under it (`ruby-position: under`).

![Japanese text with small kana readings over some words and red sesame marks over others, and a line with blue dots above and readings below.](showcase/ruby-emphasis.png)

## Floats

A picture floats right and a three-line initial letter sinks at the start
of the same lines, with justified text between them. Below, Japanese text
with ruby flows beside a float on the left.

![A justified paragraph with a red initial F on the left and a landscape picture on the right, then Japanese text with readings beside a pink box.](showcase/floats.png)

## Vertical text

Japanese set in `vertical-rl`, with ruby to the right of the lines and
emphasis marks beside them. `text-combine-upright: digits 2` sets runs of
up to two digits upright in one em, as in dates and times, and leaves the
four-digit number on its side. Chrome does not support `digits`.

![Vertical Japanese columns read from right to left, with readings beside some characters, red sesame marks, and one- and two-digit numbers set upright in a single character's space.](showcase/vertical.png)

## Text fit

`text-fit` scales a line's text so the line fills its box. Above,
preformatted lines of different lengths each grow to the full measure under
`grow per-line-all`. Below, `grow` and `shrink` take one scale for the whole
block: short lines grow until the longest fills the box, and overlong lines
shrink until the longest fits.

![A poster of four blue lines in Palatino, each scaled to fill the same width, so the short last line is set largest; below, two blocks of three lines, one grown and one shrunk by a single scale until its longest line meets the edge.](showcase/text-fit.png)
