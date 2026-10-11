# Text editor

A desktop plain-text editor using Winkin, Fontwich, Vello CPU, Winit and
Softbuffer. It uses the system UI font, a dark theme and no GPU. No Parley
dependency is required.

Run from the workspace root:

```sh
cargo run --release -p winkin_editor
cargo run --release -p winkin_editor -- path/to/document.txt
```

Without a file, the editor opens sample text covering whitespace, combining
marks, emoji, CJK and bidirectional text. An opened file can be saved with
Cmd/Ctrl+S. The example reads UTF-8 and normalizes line endings to LF.

| Input | Action |
|---|---|
| Click or drag | Place the caret or select text |
| Double-click, then hold and drag | Select and extend by whole words |
| Triple-click, then hold and drag | Select and extend by whole wrapped lines |
| Shift+click | Extend the selection |
| Left / Right | Move by character in visual order |
| Up / Down | Move by wrapped line, retaining the target column |
| Shift+navigation | Extend the selection |
| Option+Left/Right on macOS; Ctrl+Left/Right elsewhere | Move by word |
| Home / End | Move to the start or end of the wrapped line |
| Cmd+Left/Right on macOS | Move to the start or end of the wrapped line |
| Cmd+Up/Down on macOS; Ctrl+Home/End elsewhere | Move to the start or end of the document |
| Backspace / Delete | Delete backward or forward; add the word modifier to delete a word |
| Enter / Tab | Insert a newline or tab |
| Cmd/Ctrl+A, C, X, V | Select all, copy, cut, paste |
| Cmd/Ctrl+Z | Undo |
| Cmd+Shift+Z on macOS; Ctrl+Y or Ctrl+Shift+Z elsewhere | Redo |
| Mouse wheel / trackpad | Scroll vertically |
| Escape | Cancel IME composition |

## Box edges and style seams

Text between `[` and `]` is laid out in an inline box with 10px of padding
at each end and a 3px border, drawn behind the text. Text between `{` and
`}` is a bold span with no room at its ends. The brackets stay in the
buffer but take no room themselves, so typed text visibly lands inside or
outside a box or span. Each text segment between brackets is a node of its
own. The window title shows the focus: its layout offset, its affinity, and
the node and offset it maps to.

A caret keeps the side it arrived from. Right (forward) lands upstream, at
the end of the text it passed; Left (backward) lands downstream, at the
start of the text it came from. A click takes the side of the glyph half it
hits. Typed text goes into the node on that side: at the seam between
normal and bold text, arriving from the normal side types normal text, and
arriving from the bold side types bold text. Without room the two sides
are drawn at one place, and only the title shows which one the caret has.

A box edge with padding draws the two sides apart. Clicks in the padding
take the nearer side. Arrows stop once at each edge, on the side they come
from, and the next press crosses the padding and the next character
together. To try both:

```sh
cargo run --release -p winkin_editor -- examples/editor/box-edges.txt
```

The file holds a plain box, boxes at a line's start and end, nested boxes,
an empty box, a right-to-left line, Hebrew in a box on a left-to-right
line, and bold spans in the middle, at the start and nested.

Horizontal arrows cross line boundaries in screen order: Right continues
onto the next line from the right edge, and Left continues onto the previous
line from the left edge. This applies to both LTR and RTL paragraphs.

IME preedit text is underlined. Its cursor or selected range is drawn using
Winkin geometry, and the candidate window follows the caret. Committing a
composition creates one undo entry; cancelling restores the original text
and selection.

## Structure

- `src/editor.rs` owns the buffer, undo history and composition. Winkin
  supplies navigation, hit testing, source mapping and selection geometry.
- `src/render.rs` draws selection rectangles, positioned glyphs, composition
  underlines and the caret. Font bytes and their IDs are shared with Vello;
  the render context and font resources persist between frames.
- `src/main.rs` translates window events into editing commands, manages
  scrolling and presents CPU pixels through Softbuffer.

Text edits rebuild the whole document, retaining the layout and context
allocations. Resizing only breaks lines again. Selection and caret changes
reuse prepared layout. An editor-only terminal break supplies a line box for
an empty document or a trailing newline; it is never copied or saved.

This is an integration example, not a full editor widget. Undo stores up to
100 complete buffer snapshots, with one entry per edit. There is no
incremental paragraph layout, syntax highlighting, accessibility adapter,
file dialog or unsaved-change prompt. Closing
the window discards unsaved edits. Native window behavior has been checked
on macOS; Windows and Linux need a separate check.

## Checks

```sh
cargo test -p winkin_editor
cargo clippy -p winkin_editor --all-targets -- -D warnings
cargo run --release -p winkin_editor -- --screenshot /tmp/editor.png
```

The screenshot command renders a 900×660 image without opening a window.
An optional text-file argument supplies its content. It uses the same
renderer as the window; output varies with installed fonts.
