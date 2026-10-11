use std::ops::Range;

use winkin::selection::{Affinity, Granularity, MotionDirection, Position, Selection, WordMotion};
use winkin::style::{
    ComputedStyle, EdgesGroup, FontFamilyName, FontWeight, GenericFamily, OverflowWrap, Sides,
    UnicodeBidi, WhiteSpaceCollapse,
};
use winkin::{Area, BuildOptions, ComputedBlockStyle, Context, Layout, NoExclusions, NodeKey};

const HISTORY_LIMIT: usize = 100;
/// The first key of a `{`…`}` bold span, which has no room and is not drawn.
pub const SPAN_KEYS: u64 = 1_000_000;
const FAMILIES: &[FontFamilyName<'static>] = &[FontFamilyName::Generic(GenericFamily::SystemUi)];

#[derive(Clone)]
struct Snapshot {
    text: String,
    anchor: (usize, Affinity),
    focus: (usize, Affinity),
}

struct Composition {
    cursor_visible: bool,
    original: Snapshot,
    range: Range<usize>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SelectionUnit {
    Character,
    Word,
    Line,
}

/// A run of buffer text between brackets, laid out as one text node.
#[derive(Copy, Clone, Debug)]
struct Segment {
    key: NodeKey,
    start: usize,
    end: usize,
}

#[derive(Copy, Clone)]
struct Drag {
    start: Position,
    end: Position,
    unit: SelectionUnit,
}

pub struct Editor {
    pub text: String,
    pub layout: Layout,
    pub selection: Selection,
    segments: Vec<Segment>,
    cx: Context,
    width: f32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    composition: Option<Composition>,
    drag: Option<Drag>,
}

impl Editor {
    pub fn new(text: String) -> Self {
        let mut editor = Self {
            text: normalize(&text),
            layout: Layout::new(),
            selection: Position::from(0).into(),
            segments: Vec::new(),
            cx: Context::new(fontwich::Collection::system()),
            width: 760.0,
            undo: Vec::new(),
            redo: Vec::new(),
            composition: None,
            drag: None,
        };
        editor.rebuild((0, Affinity::Downstream), (0, Affinity::Downstream));
        editor
    }

    fn source(&self, position: Position) -> (usize, Affinity) {
        let offset = self
            .layout
            .node_position(position)
            .and_then(|node| {
                let segment = self
                    .segments
                    .iter()
                    .find(|segment| segment.key == node.key)?;
                Some(segment.start + node.offset)
            })
            .unwrap_or(self.text.len());
        (offset, position.affinity)
    }

    fn position(&self, (offset, affinity): (usize, Affinity)) -> Position {
        // The segment holding the offset; between two brackets, the one the
        // affinity leans toward.
        let holds = |segment: &&Segment| segment.start <= offset && offset <= segment.end;
        let before = || {
            self.segments
                .iter()
                .rev()
                .find(|segment| segment.end <= offset)
        };
        let after = || self.segments.iter().find(|segment| segment.start >= offset);
        let segment = match affinity {
            Affinity::Upstream => self
                .segments
                .iter()
                .find(holds)
                .or_else(before)
                .or_else(after),
            Affinity::Downstream => self
                .segments
                .iter()
                .rev()
                .find(holds)
                .or_else(after)
                .or_else(before),
        };
        segment
            .and_then(|segment| {
                let local = offset.clamp(segment.start, segment.end) - segment.start;
                self.layout.position(segment.key, local, affinity)
            })
            .unwrap_or(Position::from(0))
    }

    /// Describes the focus: its layout offset and affinity, and its node and offset there.
    pub fn focus_description(&self) -> String {
        let focus = self.selection.focus();
        let node = self
            .layout
            .node_position(focus)
            .map_or("none".to_string(), |node| {
                format!("node {}@{}", node.key.0, node.offset)
            });
        format!("{} {:?}, {node}", focus.offset, focus.affinity)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            anchor: self.source(self.selection.anchor()),
            focus: self.source(self.selection.focus()),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.rebuild(snapshot.anchor, snapshot.focus);
    }

    fn rebuild(&mut self, anchor: (usize, Affinity), focus: (usize, Affinity)) {
        self.drag = None;
        let mut style = ComputedStyle::initial();
        style.font.families = FAMILIES;
        style.font.size = 20.0;
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        style.text.overflow_wrap = OverflowWrap::Anywhere;
        // A box for each `[`…`]`: 10px of padding at its ends and a 3px border.
        let boxed = ComputedStyle {
            edges: EdgesGroup {
                border: Sides::all(3.0),
                padding: Sides::<f32> {
                    left: 10.0,
                    right: 10.0,
                    ..Sides::ZERO
                }
                .into(),
                ..EdgesGroup::INITIAL
            },
            ..style
        };
        // A bold span for each `{`…`}`, with no room at its ends.
        let mut bold = style;
        bold.font.weight = FontWeight::BOLD;
        style.bidi.unicode_bidi = UnicodeBidi::Plaintext;
        let block = ComputedBlockStyle::new(&style);
        let mut options = BuildOptions::default();
        options.map_source = true;
        let mut builder = self.layout.builder(NodeKey(0), &block, options);
        // The text between brackets goes in segments, each a node of its own.
        // The brackets stay in the buffer and take no text in the layout. A
        // closing bracket closes only the innermost open one of its kind.
        let mut segments = Vec::new();
        let mut boxes = 0;
        let mut open = Vec::new();
        let mut start = 0;
        let mut flush = |builder: &mut winkin::LayoutBuilder<'_>, start: usize, end: usize| {
            if start < end {
                let key = NodeKey(1 + segments.len() as u64);
                builder.text(key, &self.text[start..end]);
                segments.push(Segment { key, start, end });
            }
        };
        for (at, character) in self.text.char_indices() {
            match character {
                '[' | '{' => {
                    flush(&mut builder, start, at);
                    boxes += 1;
                    if character == '[' {
                        builder.open_box(NodeKey(10_000 + boxes), &boxed, None);
                    } else {
                        builder.open_box(NodeKey(SPAN_KEYS + boxes), &bold, None);
                    }
                    open.push(character);
                }
                ']' | '}' if open.last() == Some(&if character == ']' { '[' } else { '{' }) => {
                    flush(&mut builder, start, at);
                    builder.close_box();
                    open.pop();
                }
                _ => continue,
            }
            start = at + 1;
        }
        flush(&mut builder, start, self.text.len());
        for _ in open {
            builder.close_box();
        }
        // A terminal break gives empty editor paragraphs a line box.
        if self.text.is_empty() || self.text.ends_with('\n') {
            builder.line_break(NodeKey(20_000));
        }
        assert!(builder.finish(&mut self.cx).is_complete());
        self.segments = segments;
        self.layout
            .break_lines(&mut self.cx, Area::new(self.width), &mut NoExclusions);
        self.selection = Selection::new(self.position(anchor), self.position(focus));
    }

    pub fn resize(&mut self, width: f32) {
        let width = width.max(1.0);
        if width != self.width {
            self.width = width;
            self.layout
                .break_lines(&mut self.cx, Area::new(width), &mut NoExclusions);
        }
    }

    pub fn source_range(&self) -> Range<usize> {
        let anchor = self.source(self.selection.anchor()).0;
        let focus = self.source(self.selection.focus()).0;
        anchor.min(focus)..anchor.max(focus)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.source_range()]
    }

    fn remember(&mut self, snapshot: Snapshot) {
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.redo.clear();
    }

    pub fn insert(&mut self, text: &str) {
        self.cancel_composition();
        let text = normalize(text);
        let range = self.source_range();
        if range.is_empty() && text.is_empty() {
            return;
        }
        self.replace(range, &text, self.snapshot());
    }

    fn replace(&mut self, range: Range<usize>, text: &str, before: Snapshot) {
        self.remember(before);
        let offset = range.start + text.len();
        self.text.replace_range(range, text);
        self.rebuild(
            (offset, Affinity::Downstream),
            (offset, Affinity::Downstream),
        );
    }

    pub fn delete(&mut self, direction: MotionDirection, word: bool) {
        self.cancel_composition();
        let before = self.snapshot();
        if self.selection.is_collapsed() {
            self.selection.modify(
                &self.layout,
                direction.extending(if word {
                    Granularity::Word
                } else {
                    Granularity::Character
                }),
            );
        }
        let range = self.source_range();
        if range.is_empty() {
            self.selection =
                Selection::new(self.position(before.anchor), self.position(before.focus));
        } else {
            self.replace(range, "", before);
        }
    }

    pub fn navigate(&mut self, direction: MotionDirection, granularity: Granularity, extend: bool) {
        self.cancel_composition();
        let motion = if extend {
            direction.extending(granularity)
        } else {
            direction.moving(granularity)
        };
        self.selection.modify(&self.layout, motion);
    }

    pub fn select_all(&mut self) {
        self.cancel_composition();
        self.selection = Selection::new(
            self.position((0, Affinity::Downstream)),
            self.position((self.text.len(), Affinity::Upstream)),
        );
    }

    pub fn pointer_unit(&mut self, x: f32, y: f32, unit: SelectionUnit, extend: bool) {
        self.cancel_composition();
        let Some(position) = self
            .layout
            .hit_test(x, y, winkin::config::PastLines::platform())
        else {
            return;
        };
        let (start, end) = self.pointer_range(position, x, unit);
        self.drag = Some(if extend {
            let anchor = self.selection.anchor();
            Drag {
                start: anchor,
                end: anchor,
                unit,
            }
        } else {
            Drag { start, end, unit }
        });
        self.extend_drag(start, end);
    }

    pub fn drag_pointer(&mut self, x: f32, y: f32) {
        let Some(drag) = self.drag else { return };
        if let Some(position) = self
            .layout
            .hit_test(x, y, winkin::config::PastLines::platform())
        {
            let (start, end) = self.pointer_range(position, x, drag.unit);
            self.extend_drag(start, end);
        }
    }

    fn extend_drag(&mut self, start: Position, end: Position) {
        let Some(origin) = self.drag else { return };
        self.selection = if origin.unit == SelectionUnit::Character {
            Selection::new(origin.start, start)
        } else if start.offset < origin.start.offset {
            Selection::new(origin.end, start)
        } else if end.offset > origin.end.offset {
            Selection::new(origin.start, end)
        } else {
            Selection::new(origin.start, origin.end)
        };
    }

    fn pointer_range(
        &self,
        position: Position,
        x: f32,
        unit: SelectionUnit,
    ) -> (Position, Position) {
        match unit {
            SelectionUnit::Character => (position, position),
            SelectionUnit::Line => {
                let Some(caret) = self.layout.caret(position) else {
                    return (position, position);
                };
                let range = self.layout.line(caret.line).unwrap().text_range();
                let limit = self.position((self.text.len(), Affinity::Upstream)).offset;
                (
                    Position::new(range.start.min(limit), Affinity::Downstream),
                    Position::new(range.end.min(limit), Affinity::Upstream),
                )
            }
            SelectionUnit::Word => {
                let Some(carets) = self.layout.carets(position) else {
                    return (position, position);
                };
                let distance = |caret: &winkin::selection::Caret| {
                    let line = self.layout.line(caret.line).unwrap().metrics();
                    (line.left + caret.inline.left - x).abs()
                };
                let caret = carets
                    .weak
                    .filter(|weak| distance(weak) < distance(&carets.strong))
                    .unwrap_or(carets.strong);
                let line = self.layout.line(caret.line).unwrap();
                // A bidi boundary can map the insertion caret to another run.
                // Select the text under the pointer before interpreting that caret.
                let inline = x - line.metrics().left;
                for item in line.items() {
                    let winkin::Item::Text(run) = item else {
                        continue;
                    };
                    for cluster in run.clusters() {
                        let extent = cluster.inline();
                        if extent.left <= inline && inline < extent.right {
                            let source = self.source(Position::from(cluster.text_range().start)).0;
                            return self.word_range(source);
                        }
                    }
                }
                let before = (x < line.metrics().left + caret.inline.left) != caret.rtl;
                let mut clicked = Selection::from(position);
                if before {
                    clicked.modify(
                        &self.layout,
                        MotionDirection::Backward.moving(Granularity::Character),
                    );
                }
                let range = line.text_range();
                let source = self.source(clicked.focus()).0;
                let start = self.source(Position::from(range.start)).0;
                let end = self.source(Position::new(range.end, Affinity::Upstream)).0;
                let mut offset = source.max(start);
                if offset >= end && end > start {
                    offset = self.text[..end].char_indices().next_back().unwrap().0;
                }
                self.word_range(offset)
            }
        }
    }

    fn word_range(&self, offset: usize) -> (Position, Position) {
        let offset = offset.min(self.text.len());
        let position = self.position((offset, Affinity::Downstream));
        let Some(character) = self.text[offset..].chars().next() else {
            return (position, position);
        };
        if character.is_whitespace() {
            let mut start = offset;
            let mut end = offset + character.len_utf8();
            if character != '\n' {
                for (index, ch) in self.text[..offset].char_indices().rev() {
                    if !ch.is_whitespace() || ch == '\n' {
                        break;
                    }
                    start = index;
                }
                for ch in self.text[end..].chars() {
                    if !ch.is_whitespace() || ch == '\n' {
                        break;
                    }
                    end += ch.len_utf8();
                }
            }
            return (
                self.position((start, Affinity::Downstream)),
                self.position((end, Affinity::Upstream)),
            );
        }
        // Both boundaries use Winkin's word rules, including dictionary segmentation.
        let mut end = Selection::from(position);
        end.modify(
            &self.layout,
            MotionDirection::Forward
                .moving(Granularity::Word)
                .with_word_motion(WordMotion::StopAtWordEnd),
        );
        let mut start = end;
        start.modify(
            &self.layout,
            MotionDirection::Backward.moving(Granularity::Word),
        );
        (start.focus(), end.focus())
    }

    pub fn undo(&mut self) {
        self.cancel_composition();
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    pub fn redo(&mut self) {
        self.cancel_composition();
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    pub fn preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.cancel_composition();
            return;
        }
        let composition = self.composition.take().unwrap_or_else(|| Composition {
            cursor_visible: true,
            original: self.snapshot(),
            range: self.source_range(),
        });
        let start = composition.range.start;
        let preedit = text;
        let text = normalize(preedit);
        self.text.replace_range(composition.range, &text);
        let (anchor, focus) = cursor.unwrap_or((preedit.len(), preedit.len()));
        // IME offsets count bytes in the unnormalized preedit string.
        let boundary = |offset: usize| {
            let mut offset = offset.min(preedit.len());
            while !preedit.is_char_boundary(offset) {
                offset -= 1;
            }
            start + normalize(&preedit[..offset]).len()
        };
        self.rebuild(
            (boundary(anchor), Affinity::Downstream),
            (boundary(focus), Affinity::Downstream),
        );
        self.composition = Some(Composition {
            cursor_visible: cursor.is_some(),
            original: composition.original,
            range: start..start + text.len(),
        });
    }

    pub fn caret_visible(&self) -> bool {
        self.composition
            .as_ref()
            .is_none_or(|composition| composition.cursor_visible)
    }

    pub fn composing(&self) -> bool {
        self.composition.is_some()
    }

    pub fn composition_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|composition| {
            self.position((composition.range.start, Affinity::Downstream))
                .offset
                ..self
                    .position((composition.range.end, Affinity::Upstream))
                    .offset
        })
    }

    pub fn cancel_composition(&mut self) {
        if let Some(composition) = self.composition.take() {
            self.restore(composition.original);
        }
    }
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(editor: &Editor, offset: usize) -> (f32, f32) {
        let position = editor.position((offset, Affinity::Downstream));
        let caret = editor.layout.caret(position).unwrap();
        let line = editor.layout.line(caret.line).unwrap().metrics();
        let mut next = Selection::from(position);
        next.modify(
            &editor.layout,
            MotionDirection::Forward.moving(Granularity::Character),
        );
        let after = editor.layout.caret(next.focus()).unwrap();
        (
            line.left + (caret.inline.left + after.inline.left) * 0.5,
            line.top + (caret.block.over + caret.block.under) * 0.5,
        )
    }

    #[test]
    fn word_drag_keeps_whole_words_and_reverses_around_the_original_word() {
        let mut editor = Editor::new("one two three four".into());
        let (x, y) = point(&editor, 5);
        editor.pointer_unit(x, y, SelectionUnit::Word, false);
        assert_eq!(editor.selected_text(), "two");
        let (x, y) = point(&editor, 15);
        editor.drag_pointer(x, y);
        assert_eq!(editor.selected_text(), "two three four");
        let (x, y) = point(&editor, 1);
        editor.drag_pointer(x, y);
        assert_eq!(editor.selected_text(), "one two");
        assert!(editor.selection.focus().offset < editor.selection.anchor().offset);
        let (x, y) = point(&editor, 6);
        editor.drag_pointer(x, y);
        assert_eq!(editor.selected_text(), "two");
    }

    #[test]
    fn double_click_tracks_the_cluster_at_bidi_boundaries() {
        for text in [
            "English עברית 123.",
            "Mixed directions: English العربية English עברית 123.",
            "עברית 123 English",
        ] {
            let mut editor = Editor::new(text.into());
            let mut clicks = Vec::new();
            for line in editor.layout.lines() {
                let metrics = line.metrics();
                for item in line.items() {
                    let winkin::Item::Text(run) = item else {
                        continue;
                    };
                    let block = run.block();
                    for cluster in run.clusters() {
                        let inline = cluster.inline();
                        if inline.right <= inline.left {
                            continue;
                        }
                        let source = editor.source(Position::from(cluster.text_range().start)).0;
                        let (start, end) = editor.word_range(source);
                        let expected = editor.source(start).0..editor.source(end).0;
                        for fraction in [0.01, 0.25, 0.5, 0.75, 0.99] {
                            clicks.push((
                                metrics.left + inline.left + cluster.advance() * fraction,
                                metrics.top + (block.over + block.under) * 0.5,
                                expected.clone(),
                                source,
                                fraction,
                            ));
                        }
                    }
                }
            }
            for (x, y, expected, source, fraction) in clicks {
                editor.pointer_unit(x, y, SelectionUnit::Word, false);
                assert_eq!(
                    editor.source_range(),
                    expected,
                    "{text:?}: cluster at {source}, fraction {fraction}"
                );
            }
        }
    }

    #[test]
    fn word_selection_uses_unicode_and_winkin_punctuation_rules() {
        let editor = Editor::new("café עברית x.y 3.14   end".into());
        for (needle, expected) in [
            ("fé", "café"),
            ("רית", "עברית"),
            ("x", "x"),
            (".y", "."),
            ("14", "3.14"),
            ("   ", "   "),
        ] {
            let offset = editor.text.find(needle).unwrap();
            let (start, end) = editor.word_range(offset);
            let selected = &editor.text[editor.source(start).0..editor.source(end).0];
            assert_eq!(selected, expected, "at {needle}");
        }
    }

    #[test]
    fn line_drag_selects_wrapped_lines_and_hard_line_breaks() {
        let mut editor = Editor::new("first line\nsecond line\nlast".into());
        let (x, y) = point(&editor, 13);
        editor.pointer_unit(x, y, SelectionUnit::Line, false);
        assert_eq!(editor.selected_text(), "second line\n");
        let (x, y) = point(&editor, 2);
        editor.drag_pointer(x, y);
        assert_eq!(editor.selected_text(), "first line\nsecond line\n");
        let (x, y) = point(&editor, editor.text.len() - 2);
        editor.drag_pointer(x, y);
        assert_eq!(editor.selected_text(), "second line\nlast");

        editor.resize(70.0);
        let position = editor.position((2, Affinity::Downstream));
        let caret = editor.layout.caret(position).unwrap();
        let range = editor.layout.line(caret.line).unwrap().text_range();
        let (x, y) = point(&editor, 2);
        editor.pointer_unit(x, y, SelectionUnit::Line, false);
        assert_eq!(editor.selection.range(), range);
    }

    #[test]
    fn right_arrow_in_a_mixed_ltr_line_stays_on_that_line() {
        let mut editor = Editor::new("previous\nEnglish עברית 123.\nnext".into());
        let start = editor.text.find("English").unwrap();
        let end = editor.text.rfind("\nnext").unwrap();
        for offset in start..end {
            if !editor.text.is_char_boundary(offset) {
                continue;
            }
            editor.selection = editor.position((offset, Affinity::Downstream)).into();
            let before = editor.layout.caret(editor.selection.focus()).unwrap();
            editor.navigate(MotionDirection::Right, Granularity::Character, false);
            let after = editor.layout.caret(editor.selection.focus()).unwrap();
            assert!(
                after.line >= before.line,
                "from byte {offset}: {before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn right_arrow_leaves_an_rtl_paragraph_onto_the_next_line() {
        let mut editor = Editor::new("previous\nעברית\nnext".into());
        let offset = "previous\n".len();
        editor.selection = editor.position((offset, Affinity::Downstream)).into();
        let before = editor.layout.caret(editor.selection.focus()).unwrap();
        editor.navigate(MotionDirection::Right, Granularity::Character, false);
        let after = editor.layout.caret(editor.selection.focus()).unwrap();
        assert_eq!(before.line, 1);
        assert_eq!(after.line, 2);
        assert_eq!(
            editor.source_range().start,
            editor.text.find("next").unwrap()
        );
    }

    #[test]
    fn typing_at_a_style_seam_goes_into_the_node_the_caret_came_from() {
        let mut editor = Editor::new("ab{cd}e".into());
        let step = |editor: &mut Editor, direction: MotionDirection| {
            editor.navigate(direction, Granularity::Character, false)
        };
        // Forward to the seam after `b`: the normal text before it.
        step(&mut editor, MotionDirection::Forward);
        step(&mut editor, MotionDirection::Forward);
        assert_eq!(editor.focus_description(), "2 Upstream, node 1@2");
        editor.insert("x");
        assert_eq!(editor.focus_description(), "3 Upstream, node 1@3");
        editor.insert("y");
        assert_eq!(editor.text, "abxy{cd}e");
        // Back to the same seam from the bold text: the bold text after it.
        step(&mut editor, MotionDirection::Forward);
        step(&mut editor, MotionDirection::Backward);
        assert_eq!(editor.focus_description(), "4 Downstream, node 2@0");
        editor.insert("z");
        assert_eq!(editor.text, "abxy{zcd}e");
        // Left from the end to the closing seam: the normal text after it.
        editor.navigate(
            MotionDirection::Forward,
            Granularity::DocumentBoundary,
            false,
        );
        step(&mut editor, MotionDirection::Left);
        editor.insert("w");
        assert_eq!(editor.text, "abxy{zcd}we");
        // Right onto the closing seam from the bold text: the bold text.
        step(&mut editor, MotionDirection::Left);
        step(&mut editor, MotionDirection::Left);
        step(&mut editor, MotionDirection::Right);
        editor.insert("v");
        assert_eq!(editor.text, "abxy{zcdv}we");
    }

    #[test]
    fn typing_at_a_box_edge_goes_on_the_side_the_caret_is_drawn() {
        let mut editor = Editor::new("a[bc]d".into());
        let forward = |editor: &mut Editor| {
            editor.navigate(MotionDirection::Forward, Granularity::Character, false)
        };
        let backward = |editor: &mut Editor| {
            editor.navigate(MotionDirection::Backward, Granularity::Character, false)
        };
        // Forward stops after `a`, outside the box.
        forward(&mut editor);
        assert_eq!(editor.focus_description(), "1 Upstream, node 1@1");
        editor.insert("x");
        assert_eq!(editor.text, "ax[bc]d");
        // The next step crosses the padding and `b` together.
        forward(&mut editor);
        editor.insert("y");
        assert_eq!(editor.text, "ax[byc]d");
        // Backward stops before `b`, inside the box.
        backward(&mut editor);
        backward(&mut editor);
        assert_eq!(editor.focus_description(), "2 Downstream, node 2@0");
        editor.insert("z");
        assert_eq!(editor.text, "ax[zbyc]d");
        // Forward to the closing edge stops inside; backward from `d`, outside.
        for _ in 0..3 {
            forward(&mut editor);
        }
        editor.insert("w");
        assert_eq!(editor.text, "ax[zbycw]d");
        editor.navigate(
            MotionDirection::Forward,
            Granularity::DocumentBoundary,
            false,
        );
        backward(&mut editor);
        editor.insert("v");
        assert_eq!(editor.text, "ax[zbycw]vd");
    }

    #[test]
    fn edits_replace_selection_and_undo_restores_it() {
        let mut editor = Editor::new("hello 世界".into());
        editor.select_all();
        editor.insert("café");
        assert_eq!(editor.text, "café");
        assert_eq!(editor.source_range(), 5..5);
        editor.undo();
        assert_eq!(editor.selected_text(), "hello 世界");
        editor.redo();
        assert_eq!(editor.text, "café");
    }

    #[test]
    fn backspace_removes_a_grapheme() {
        let mut editor = Editor::new("ae\u{301}".into());
        editor.navigate(
            MotionDirection::Forward,
            Granularity::DocumentBoundary,
            false,
        );
        editor.delete(MotionDirection::Backward, false);
        assert_eq!(editor.text, "a");
    }

    #[test]
    fn undo_after_backspace_restores_a_caret_and_family_emoji() {
        let mut editor = Editor::new("a👩‍👩‍👧‍👦".into());
        editor.navigate(
            MotionDirection::Forward,
            Granularity::DocumentBoundary,
            false,
        );
        let before = editor.source_range();
        editor.delete(MotionDirection::Backward, false);
        assert_eq!(editor.text, "a");
        editor.undo();
        assert_eq!(editor.text, "a👩‍👩‍👧‍👦");
        assert_eq!(editor.source_range(), before);
        assert!(editor.selection.is_collapsed());
    }

    #[test]
    fn clicking_the_terminal_line_does_not_insert_the_editor_break() {
        let mut editor = Editor::new("abc\n".into());
        editor.pointer_unit(0.0, 1000.0, SelectionUnit::Character, false);
        editor.insert("end");
        assert_eq!(editor.text, "abc\nend");
        editor.select_all();
        assert_eq!(editor.selected_text(), "abc\nend");
    }

    #[test]
    fn preedit_is_replaced_cancelled_and_committed_as_one_edit() {
        let mut editor = Editor::new("abc".into());
        editor.select_all();
        editor.preedit("に", Some((3, 3)));
        editor.preedit("日本", Some((6, 6)));
        assert_eq!(editor.text, "日本");
        editor.cancel_composition();
        assert_eq!(editor.selected_text(), "abc");
        editor.preedit("日本", None);
        editor.insert("日本語");
        assert_eq!(editor.text, "日本語");
        editor.undo();
        assert_eq!(editor.selected_text(), "abc");
    }

    #[test]
    fn empty_text_newlines_and_reflow_keep_positions_valid() {
        let mut editor = Editor::new(String::new());
        assert!(editor.layout.caret(editor.selection.focus()).is_some());
        editor.insert("a\r\nאבג\n");
        assert_eq!(editor.text, "a\nאבג\n");
        let range = editor.source_range();
        editor.resize(20.0);
        assert_eq!(editor.source_range(), range);
        assert!(editor.layout.caret(editor.selection.focus()).is_some());
        editor.delete(MotionDirection::Backward, false);
        assert_eq!(editor.text, "a\nאבג");
    }
}
