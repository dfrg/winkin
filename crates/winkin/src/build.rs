//! The builder, through which all content and styles enter a layout.
//!
//! In: text, inline boxes, atomic inlines, floats, absolutely positioned
//! boxes, breaks and ruby, in document order. Out: a prepared layout and a
//! [`BuildReport`] of what was not kept.
//! Start at: [`LayoutBuilder::text`] and [`LayoutBuilder::open_box`], then
//! [`LayoutBuilder::finish`].
//!
//! An engine drives the builder from its tree. A simple layout makes the same
//! calls, fewer of them. Every call goes straight to the content's writer.
//! The writer collapses white space, lowers each style into facts and checks
//! every limit as it goes. The builder holds nothing but the writer.
//!
//! **Nothing panics, and nothing fails.** Text past the content's limit, and
//! nodes past what their ids can name, are dropped where they arrive. They
//! drop whole characters and whole nodes at a time.
//! [`finish`](LayoutBuilder::finish) counts them in its [`BuildReport`]. An
//! unbalanced close is ignored. Whatever is still open when the build
//! finishes is closed, ruby included. A builder dropped without finishing
//! closes its content the same way, so a layout is never left half built.

use core::hash::{Hash, Hasher};

use crate::context::Context;
use crate::font::FontMetricsProvider;
use crate::layout::PreparedStages;
use crate::stages::analysis::BidiLevel;
use crate::stages::content::NodeKey;
use crate::stages::content::{ContainerKind, ContentWriter};
use crate::style::{ComputedStyle, RubyPosition};

/// Content dropped or replaced during a build.
///
/// Returned by [`LayoutBuilder::finish`]. Exceeding a layout limit does not
/// fail or panic; the layout contains the retained content.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct BuildReport {
    /// The number of input text bytes dropped.
    ///
    /// Text is dropped when the content reaches 2^30 − 1 bytes or its node
    /// cannot be created.
    pub dropped_bytes: usize,
    /// The number of nodes dropped because a table was full.
    ///
    /// Includes boxes, text nodes, atomic inlines, floats, absolutely
    /// positioned boxes, breaks, ruby containers and annotations, along with
    /// their contents.
    pub dropped_nodes: usize,
    /// The number of styles replaced because a table was full.
    ///
    /// Includes styles, their lists and their resolved properties. Lists
    /// fall back to their initial values. Each kind of resolved property
    /// (text, shaping, font selection and box properties) allows up to
    /// 65,535 distinct values. Beyond this limit, text inherits the enclosing
    /// box properties and boxes use invisible, edgeless defaults.
    /// Content with this many distinct font requests may reach the font
    /// limit first.
    pub replaced_styles: usize,
    /// The number of font substitutions caused by a full font table.
    ///
    /// A layout holds up to 65,535 distinct font and size combinations.
    /// Additional fonts fall back to the primary font for the style.
    /// An additional primary font falls back to the last stored font.
    pub replaced_fonts: usize,
    /// The number of shaped glyphs dropped.
    ///
    /// A layout holds up to 2^28 − 1 additional glyphs beyond the first glyph
    /// of each cluster. Affected clusters retain their width but draw nothing.
    pub dropped_glyphs: usize,
}

impl BuildReport {
    /// Returns `true` if no content was dropped or replaced.
    pub fn is_complete(&self) -> bool {
        *self == Self::default()
    }

    /// Counts `bytes` of text dropped.
    ///
    /// The counts saturate: a 32-bit host may go on pushing text long after
    /// the content is full.
    pub(crate) fn drop_bytes(&mut self, bytes: usize) {
        self.dropped_bytes = self.dropped_bytes.saturating_add(bytes);
    }

    /// Counts a node dropped.
    pub(crate) fn drop_node(&mut self) {
        self.dropped_nodes = self.dropped_nodes.saturating_add(1);
    }

    /// Counts a style, or a list in one, replaced.
    pub(crate) fn replace_style(&mut self) {
        self.replaced_styles = self.replaced_styles.saturating_add(1);
    }
}

/// The border-box size of an atomic inline or float.
///
/// The host must lay out the box before adding it. Margins are specified
/// separately in the style.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct BoxSize {
    /// The border box's size along the line, in pixels.
    pub inline: f32,
    /// The border box's size across the line, in pixels.
    pub block: f32,
    /// The baseline offset from the block-start border edge, in pixels.
    ///
    /// If `None`, alignment uses the block-end margin edge. This applies to
    /// replaced elements and inline-blocks without a line box.
    pub baseline: Option<f32>,
}

impl BoxSize {
    /// Returns the size as the content keeps it.
    ///
    /// A size that is not finite, or is negative, becomes zero. A baseline that
    /// is not finite becomes none.
    pub(crate) fn sanitized(self) -> Self {
        let size = |n: f32| if n.is_finite() && n > 0.0 { n } else { 0.0 };
        Self {
            inline: size(self.inline),
            block: size(self.block),
            baseline: self.baseline.filter(|n| n.is_finite()),
        }
    }
}

/// The float side: `float: left` or `float: right`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FloatSide {
    /// The left.
    #[default]
    Left,
    /// The right.
    Right,
}

impl FloatSide {
    /// Returns the side an initial letter floats to in a paragraph at `level`.
    ///
    /// It is the paragraph's start: the right where it reads right to left,
    /// the left where it reads left to right. Chrome places the letter's
    /// exclusion where the line starts.
    pub(crate) fn from_level(level: BidiLevel) -> Self {
        if level.is_rtl() {
            Self::Right
        } else {
            Self::Left
        }
    }
}

/// The outer display of a box before absolute positioning blockifies it.
///
/// Determines the static position reported by [`Layout::static_positions`].
///
/// [`Layout::static_positions`]: crate::Layout::static_positions
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum OriginalDisplay {
    /// An inline-level box, such as `inline` or `inline-block`.
    ///
    /// Its static position follows the anchor on the line.
    #[default]
    Inline,
    /// A block-level box, such as `block` or `flex`.
    ///
    /// Its static position is at the inline-start edge of the area, below
    /// the line if in-flow content precedes the anchor.
    Block,
}

/// The floats a forced break clears: `clear` on a `<br>`.
///
/// The line after the break starts below every float on these sides.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Clear {
    /// The floats pulled to the left.
    Left,
    /// The floats pulled to the right.
    Right,
    /// The floats on both sides.
    Both,
}

impl Clear {
    /// Whether it clears the floats on `side`.
    pub(crate) fn clears(self, side: FloatSide) -> bool {
        matches!(
            (self, side),
            (Self::Both, _) | (Self::Left, FloatSide::Left) | (Self::Right, FloatSide::Right)
        )
    }
}

/// Options for a build: what to record, and the geometry to start with.
///
/// Construct with [`Default::default`] and set the required fields.
#[derive(Copy, Clone, Debug, Default)]
#[non_exhaustive]
pub struct BuildOptions {
    /// Whether to record source offsets for position conversion.
    ///
    /// The map relates layout text to the original text of each node, using
    /// a few words per text node when no collapsing or transformation occurs.
    ///
    /// Without it, positions refer only to [`Layout::text`]. For a single
    /// text node with no collapsing or transformation, these match the
    /// source offsets.
    ///
    /// [`Layout::text`]: crate::Layout::text
    pub map_source: bool,
    /// The width, in pixels, that percentage margins and padding of the
    /// boxes are taken of: the containing block's inline size.
    ///
    /// Defaults to zero. [`Layout::measure`] sets another without building
    /// again.
    ///
    /// [`Layout::measure`]: crate::Layout::measure
    pub percentage_basis: f32,
}

impl PartialEq for BuildOptions {
    fn eq(&self, other: &Self) -> bool {
        self.map_source == other.map_source
            && self.percentage_basis.to_bits() == other.percentage_basis.to_bits()
    }
}

impl Eq for BuildOptions {}

impl Hash for BuildOptions {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.map_source.hash(state);
        self.percentage_basis.to_bits().hash(state);
    }
}

/// Builds layout content in document order.
///
/// Returned by [`Layout::builder`]. Text belongs to the innermost open box.
/// Opening a container requires its computed style.
///
/// [`Layout::builder`]: crate::Layout::builder
pub struct LayoutBuilder<'a> {
    writer: ContentWriter<'a>,
    /// The layout's analysis, fonts, shaped glyphs and measurements, which
    /// finishing writes from the content.
    stages: &'a mut PreparedStages,
    /// Whether finishing prepared the layout, so dropping has nothing to
    /// finish or undo.
    prepared: bool,
}

impl<'a> LayoutBuilder<'a> {
    pub(crate) fn new(writer: ContentWriter<'a>, stages: &'a mut PreparedStages) -> Self {
        Self {
            writer,
            stages,
            prepared: false,
        }
    }

    /// Appends source text for the node identified by `key`.
    ///
    /// The builder collapses whitespace as required by the style.
    /// Consecutive text calls with the same key form one node. Reusing a key
    /// later creates a new node; it does not merge nonconsecutive nodes.
    pub fn text(&mut self, key: NodeKey, text: &str) {
        self.writer.text(key, text);
    }

    /// Opens an inline box with `key` and `style`.
    ///
    /// `first_line` supplies the properties permitted by `::first-line`;
    /// all other properties come from `style`. If `None`, the first line
    /// also uses `style`.
    pub fn open_box(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        self.writer.open(ContainerKind::Box, key, style, first_line);
    }

    /// Closes the innermost open box.
    ///
    /// Does nothing if no box is open or a ruby container or annotation is
    /// open inside it.
    pub fn close_box(&mut self) {
        self.writer.close();
    }

    /// Appends an atomic inline, such as an inline-block or replaced element.
    ///
    /// `size` specifies the border box. `style` supplies margins and alignment.
    /// The inline is represented by U+FFFC in the layout text, with a caret
    /// position on each side and a bidi class. It ends a whitespace sequence.
    pub fn atomic(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        size: BoxSize,
    ) {
        self.writer.atomic(key, style, first_line, size.sanitized());
    }

    /// Appends a float anchor with the specified side and border-box size.
    ///
    /// The host places the float. The anchor adds no text and does not
    /// interrupt whitespace collapsing.
    pub fn float(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        side: FloatSide,
        size: BoxSize,
    ) {
        self.writer.float(key, style, side, size.sanitized());
    }

    /// Appends an anchor for an absolutely positioned box.
    ///
    /// The anchor adds no text or width and produces no box fragment. It
    /// preserves whitespace collapsing and line break opportunities. Its
    /// line association at a break follows Chrome.
    ///
    /// `display` is the outer display before absolute positioning blockifies
    /// the box. After line breaking, [`Layout::static_positions`] reports
    /// the static position. The host lays out and positions the box.
    ///
    /// [`Layout::static_positions`]: crate::Layout::static_positions
    pub fn absolute(&mut self, key: NodeKey, display: OriginalDisplay) {
        self.writer.absolute(key, display);
    }

    /// Appends a forced line break (`<br>`).
    ///
    /// The break is represented by `\n` in the layout text. It ends the
    /// paragraph and removes adjacent collapsible whitespace.
    /// Inside ruby, it becomes a space instead, matching Chrome.
    pub fn line_break(&mut self, key: NodeKey) {
        self.writer.line_break(key, None);
    }

    /// Appends a forced line break that clears floats (`<br clear>`).
    ///
    /// The next line starts below floats on the specified sides. A final
    /// clearing break moves the block end below those floats.
    /// Otherwise behaves like [`line_break`](Self::line_break).
    /// Inside ruby, it becomes a space and clears no floats.
    pub fn line_break_clearing(&mut self, key: NodeKey, clear: Clear) {
        self.writer.line_break(key, Some(clear));
    }

    /// Appends a soft break opportunity (`<wbr>`).
    ///
    /// The opportunity is represented by U+200B in the layout text and does
    /// not interrupt whitespace collapsing.
    pub fn break_opportunity(&mut self) {
        self.writer.break_opportunity();
    }

    /// Opens a ruby container with `key` and `style`.
    ///
    /// Following content forms the base until an annotation is opened.
    /// Base text after an annotation starts the next column.
    ///
    /// The container is an inline box: its edges occupy space, `unicode-bidi`
    /// applies, and [`Layout::box_fragments`](crate::Layout::box_fragments)
    /// returns its fragments.
    ///
    /// A container nested in a base or annotation acts as an inline box.
    /// Its annotations add levels to the containing column, matching Chrome.
    pub fn open_ruby(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        self.writer
            .open(ContainerKind::Ruby, key, style, first_line);
    }

    /// Opens an annotation in the innermost ruby container.
    ///
    /// The annotation covers the base since the previous annotation or the
    /// container start. Its side comes from the container, not `style`.
    /// Ruby nested in a base retains its own annotation side. Use
    /// [`open_annotation_with_position`](Self::open_annotation_with_position)
    /// to specify a different annotation side.
    ///
    /// Closes any boxes or annotations open inside the container first.
    /// Outside ruby, creates an anonymous container with an empty base;
    /// [`close_annotation`](Self::close_annotation) closes both.
    pub fn open_annotation(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        self.writer.annotation(key, style, first_line, None);
    }

    /// Opens an annotation with an explicit position.
    ///
    /// Use the position of an annotation container (`<rtc>`) to override
    /// the annotation and ruby styles. Otherwise behaves like
    /// [`open_annotation`](Self::open_annotation). Close it with
    /// [`close_annotation`](Self::close_annotation).
    pub fn open_annotation_with_position(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
        position: RubyPosition,
    ) {
        self.writer
            .annotation(key, style, first_line, Some(position));
    }

    /// Closes the innermost annotation and its open descendants.
    ///
    /// Does nothing if the innermost ruby container has no open annotation.
    pub fn close_annotation(&mut self) {
        self.writer.end_annotation();
    }

    /// Closes the innermost ruby container and its open descendants.
    ///
    /// Does nothing if no ruby container is open.
    pub fn close_ruby(&mut self) {
        self.writer.end_ruby();
    }

    /// Sets the style and key for `::first-letter`.
    ///
    /// The first letter in subsequent text, with surrounding punctuation,
    /// forms a separate box. `first_line` supplies its first-line style,
    /// as in [`open_box`](Self::open_box); `None` uses `style`.
    ///
    /// The letter is selected according to CSS Pseudo-Elements 4:
    /// - leading whitespace remains outside the box;
    /// - leading punctuation and intervening spaces are included;
    /// - the letter is one grapheme cluster, including digits and symbols;
    /// - trailing punctuation and preceding spaces are included, except
    ///   opening punctuation and dashes.
    ///
    /// If a text node ends with punctuation, selection continues in the
    /// next node. The box contains only the portion in the first node,
    /// matching Chrome. Each portion uses its own `text-transform`; the
    /// letter uses the transform in `style`.
    ///
    /// Resolve `style` and `first_line` against the enclosing box. Calling
    /// this method again before a letter is found replaces the styles, so
    /// it may be called before each text node.
    ///
    /// Only one first letter is selected per block. Calls are ignored after
    /// selection or after an atomic inline, forced break, ruby container,
    /// or text that prevents first-letter selection.
    ///
    /// Text runs use `key` for painting the pseudo-element.
    /// [`Layout::box_fragments`] returns the resulting box.
    ///
    /// [`Layout::box_fragments`]: crate::Layout::box_fragments
    pub fn set_first_letter(
        &mut self,
        key: NodeKey,
        style: &ComputedStyle<'_>,
        first_line: Option<&ComputedStyle<'_>>,
    ) {
        self.writer.first_letter(key, style, first_line);
    }

    /// Prepares the layout and reports dropped or replaced content.
    ///
    /// Closes all open containers, then analyzes, selects fonts, shapes and
    /// measures the content using `cx`. Intrinsic sizes are available after
    /// this call; see [`Layout::intrinsic_sizes`](crate::Layout::intrinsic_sizes).
    pub fn finish(self, cx: &mut Context) -> BuildReport {
        self.finish_with_provider(cx, None)
    }

    /// Prepares the layout with the host's glyph metrics.
    ///
    /// Pass the same provider, with stable strike settings, to
    /// [`Layout::break_lines_with_metrics`](crate::Layout::break_lines_with_metrics).
    pub fn finish_with_metrics(
        self,
        cx: &mut Context,
        provider: &dyn FontMetricsProvider,
    ) -> BuildReport {
        self.finish_with_provider(cx, Some(provider))
    }

    fn finish_with_provider(
        mut self,
        cx: &mut Context,
        provider: Option<&dyn FontMetricsProvider>,
    ) -> BuildReport {
        let mut report = self.writer.finish();
        // `PreparedStages::prepare` in layout/mod.rs runs the preparation.
        self.stages
            .prepare(self.writer.content(), cx, provider, &mut report);
        self.prepared = true;
        report
    }
}

impl Drop for LayoutBuilder<'_> {
    /// Closes the content of a builder dropped without
    /// [`finish`](LayoutBuilder::finish), so the layout is valid for what was
    /// built.
    ///
    /// With no context the layout cannot be prepared. It holds no analysis,
    /// fonts or glyphs until it is built again, rather than those of other
    /// content.
    fn drop(&mut self) {
        if !self.prepared {
            self.writer.finish();
            self.stages.clear();
        }
    }
}
