# Showcase

Each image mixes several CSS features in one specimen. winkin laid out the
text inside the Blitz browser engine, with the fonts that come with
Windows 11 (Palatino Linotype, Yu Gothic and Consolas), and Blitz painted
it. The images are 560 CSS px wide at a device pixel ratio of 2. The small
grey tag above each specimen names the features it uses.

## First line and initial letter

A capital sunk three lines deep opens a justified paragraph whose first line
is set in small capitals. The first line is shaped in its own style before
the paragraph breaks, and the letter takes its room out of the lines beside
it.

![A paragraph opening with a red capital T three lines deep, its first line in blue small capitals, justified.](showcase/first-line-initial-letter.png)

## Inline boxes across lines

Rounded, padded and bordered inline boxes break across lines. With
`box-decoration-break: slice` the box is cut open where it wraps; with
`clone` each line gets its own padding, border and rounded corners. The
lower pair nests one box inside another.

![Two columns of the same text: on the left, inline boxes sliced open at each line break; on the right, each line's part of the box closed with its own border and corners.](showcase/box-decoration-break.png)

## Ruby and emphasis marks

Japanese text carries ruby readings and sesame emphasis marks over the same
lines, and the lines make room above for both. Below, emphasis dots
sit over a base whose ruby is placed under it with `ruby-position: under`.

![Japanese text with small kana readings over some words and red sesame marks over others, and a line with blue dots above and readings below.](showcase/ruby-emphasis.png)

## Floats

A picture floats right while a three-line initial letter sinks at the start
of the same lines, and the justified text runs between them. Below, Japanese
text with ruby flows beside a float on the left.

![A justified paragraph with a red initial F on the left and a landscape picture on the right, then Japanese text with readings beside a pink box.](showcase/floats.png)

## Vertical text

Japanese set in `vertical-rl`, with ruby on the right of the lines and
emphasis marks beside them. `text-combine-upright: digits 2` sets runs of
up to two digits upright within one em, as in dates and times, and leaves
the four-digit number on its side. Chrome does not support the `digits`
value.

![Vertical Japanese columns read from right to left, with readings beside some characters, red sesame marks, and one- and two-digit numbers set upright in a single character's space.](showcase/vertical.png)

## Text fit

`text-fit` scales the text of a line so that the line fills its box. Above,
preformatted lines of different lengths each grow to the full measure with
`grow per-line-all`. Below, `grow` and `shrink` take one scale for the whole
block: the short lines grow until the longest fills the box, and the lines
too long for it shrink until the longest fits.

![A poster of four blue lines in Palatino, each scaled to fill the same width, so the short last line is set largest; below, two blocks of three lines, one grown and one shrunk by a single scale until its longest line meets the edge.](showcase/text-fit.png)
