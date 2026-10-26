//! The builder, through which all content and styles enter a layout.
//!
//! In: text, inline boxes, atomic inlines, floats, absolutely positioned
//! boxes, breaks and ruby, in document order. Out: a prepared layout and a
//! [`BuildReport`] of what was not kept.
//! Start at: [`LayoutBuilder::text`] and [`LayoutBuilder::open_box`], then
//! [`LayoutBuilder::finish`].
//!
//! Each call goes straight to the content writer, which collapses white
//! space, lowers each style into facts and checks every limit as it goes.
//!
//! Nothing panics and nothing fails. Text past the content limit and nodes
//! past what their ids can name are dropped as they arrive, in whole
//! characters and whole nodes. [`finish`](LayoutBuilder::finish) counts them
//! in its [`BuildReport`]. An unbalanced close is ignored. Finishing closes
//! whatever is still open, ruby included. A builder dropped without
//! finishing closes its content the same way.

use core::hash::{Hash, Hasher};

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
pub struct BuildReport {
    /// The number of input text bytes dropped.
    ///
    /// Text is dropped once the content holds 2^30 − 1 bytes, or where its
    /// node cannot be created.
    pub dropped_bytes: usize,
    /// The number of nodes dropped because a table was full.
    ///
    /// Includes boxes, text nodes, atomic inlines, floats, absolutely
    /// positioned boxes, breaks, ruby containers and annotations, along with
    /// their contents.
    pub dropped_nodes: usize,
    /// The number of styles replaced because a table was full.
    ///
    /// Includes styles, their lists and their resolved properties. A
    /// replaced list takes its initial value. Each kind of resolved property
    /// (text, shaping, font selection and box properties) holds up to
    /// 65,535 distinct values. Past that, text takes the enclosing box's
    /// properties, and a box is invisible and has no edges. Content with
    /// this many distinct font requests may reach the font limit first.
    pub replaced_styles: usize,
    /// The number of font substitutions caused by a full font table.
    ///
    /// A layout holds up to 65,535 distinct font and size combinations.
    /// A font past the limit is replaced by the style's primary font, and a
    /// primary font past it by the last font stored.
    pub replaced_fonts: usize,
    /// The number of shaped glyphs dropped.
    ///
    /// A cluster with more than one glyph, or a glyph with an offset, keeps
    /// its glyphs in a side table of about 2^28 entries. A cluster past
    /// that keeps its width but draws nothing.
    pub dropped_glyphs: usize,
}

impl BuildReport {
    /// Returns `true` if no content was dropped or replaced.
    pub fn is_complete(&self) -> bool {
        *self == Self::default()
    }

    /// Counts `bytes` of text dropped.
    ///
    /// The counts saturate, since a 32-bit host may push text long after the
    /// content is full.
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
/// The host lays out the box before adding it. Margins come from the style.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct BoxSize {
    /// The size along the line, in CSS pixels.
    pub inline: f32,
    /// The size across the line, in CSS pixels.
    pub block: f32,
    /// The baseline's offset from the block-start border edge, in CSS pixels.
    ///
    /// `None` aligns the block-end margin edge instead, as for replaced
    /// elements and inline-blocks without a line box.
    pub baseline: Option<f32>,
}

impl BoxSize {
    /// Returns the size as the content keeps it.
    ///
    /// A size that is negative or not finite becomes zero. A baseline that is
    /// not finite becomes `None`.
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
    /// `left`.
    #[default]
    Left,
    /// `right`.
    Right,
}

impl FloatSide {
    /// Returns the side an initial letter floats to in a paragraph at `level`:
    /// the line's start.
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
    /// Its static position is the anchor's place on the line.
    #[default]
    Inline,
    /// A block-level box, such as `block` or `flex`.
    ///
    /// Its static position is at the area's inline-start edge, below the
    /// line where in-flow content precedes the anchor.
    Block,
}

/// The floats a forced break clears: `clear` on a `<br>`.
///
/// The line after the break starts below every float on these sides.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Clear {
    /// Left floats.
    Left,
    /// Right floats.
    Right,
    /// Floats on both sides.
    Both,
}

impl Clear {
    /// Returns whether it clears the floats on `side`.
    pub(crate) fn clears(self, side: FloatSide) -> bool {
        matches!(
            (self, side),
            (Self::Both, _) | (Self::Left, FloatSide::Left) | (Self::Right, FloatSide::Right)
        )
    }
}

/// Options for a build: what to record, and the starting geometry.
#[derive(Copy, Clone, Debug, Default)]
pub struct BuildOptions {
    /// Whether to record a map from layout text to source offsets.
    ///
    /// The map relates the layout text to each node's original text. It
    /// takes a few words per text node where nothing collapses or
    /// transforms. Defaults to `false`.
    ///
    /// Without it, positions refer only to [`Layout::text`]. For a single
    /// text node where nothing collapses or transforms, these equal the
    /// source offsets.
    ///
    /// [`Layout::text`]: crate::Layout::text
    pub map_source: bool,
    /// The basis of percentage margins and padding: the containing block's
    /// inline size, in CSS pixels.
    ///
    /// Defaults to zero. [`Layout::measure`] sets another without a rebuild.
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
/// Opening a container takes its computed style. The builder borrows the
/// layout and the [`Context`](crate::Context) until it finishes or is
/// dropped.
///
/// [`Layout::builder`]: crate::Layout::builder
pub struct LayoutBuilder<'a> {
    writer: ContentWriter<'a>,
    /// The layout's analysis, fonts, shaped glyphs and measurements, written
    /// when the builder finishes.
    stages: &'a mut PreparedStages,
    /// Whether the layout is prepared, leaving nothing for `drop` to do.
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
    /// White space collapses as the style sets. Consecutive calls with the
    /// same key form one node. A key used again later makes a new node.
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
    /// `size` is the border box. `style` supplies margins and alignment.
    /// The layout text holds U+FFFC for the inline, which has a caret
    /// position on each side and takes part in bidi. It ends a run of
    /// collapsible white space.
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
    /// interrupt white space collapsing.
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
    /// The anchor adds no text or width and makes no box fragment. White
    /// space collapsing and break opportunities pass through it. An anchor
    /// at a line break stays on the line before the break, unless that line
    /// overflows or balancing or `pretty` wrapping chose its end; then it
    /// moves to the next line.
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
    /// The layout text holds `\n` for the break. It ends the paragraph and
    /// removes the collapsible white space beside it. Inside ruby it is a
    /// space instead.
    pub fn line_break(&mut self, key: NodeKey) {
        self.writer.line_break(key, None);
    }

    /// Appends a forced line break that clears floats (`<br clear>`).
    ///
    /// The next line starts below the floats on the sides `clear` names. A
    /// final clearing break moves the block end below those floats.
    /// Otherwise behaves like [`line_break`](Self::line_break). Inside ruby
    /// it is a space and clears nothing.
    pub fn line_break_clearing(&mut self, key: NodeKey, clear: Clear) {
        self.writer.line_break(key, Some(clear));
    }

    /// Appends a soft break opportunity (`<wbr>`).
    ///
    /// The layout text holds U+200B for the opportunity. It does not
    /// interrupt white space collapsing.
    pub fn break_opportunity(&mut self) {
        self.writer.break_opportunity();
    }

    /// Opens a ruby container with `key` and `style`.
    ///
    /// Content forms the base until an annotation opens. Base content after
    /// an annotation starts the next column;
    /// [`start_column`](Self::start_column) starts one over an empty base.
    ///
    /// The container is an inline box: its edges take room, `unicode-bidi`
    /// applies, and [`Layout::box_fragments`](crate::Layout::box_fragments)
    /// returns its fragments.
    ///
    /// A container nested in a base or an annotation is an inline box there.
    /// Its annotations add levels to the enclosing column.
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
    /// container start. Its side comes from the container's style, not
    /// `style`. Ruby nested in a base keeps its own side.
    /// [`open_annotation_with_position`](Self::open_annotation_with_position)
    /// sets another side.
    ///
    /// Closes any annotation open inside the container first, with the boxes
    /// inside it. Outside ruby, or inside a box in a base, opens an anonymous
    /// container with an empty base, as CSS Ruby wraps an annotation whose
    /// parent is no ruby container.
    /// [`close_annotation`](Self::close_annotation) closes it with the
    /// annotation.
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
    /// `position` overrides the side the container's style sets, as an
    /// annotation container (`<rtc>`) does. Otherwise behaves like
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

    /// Starts the next column of the innermost ruby container.
    ///
    /// Base text after an annotation starts a column by itself. With none,
    /// the next annotation would stack on the last column as another level;
    /// after this call it sits beside it, over an empty base, as CSS Ruby
    /// pairs an annotation that has no base of its own. Does nothing unless
    /// the innermost open container is a ruby container with an annotation
    /// closed in it.
    pub fn start_column(&mut self) {
        self.writer.start_column();
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
    /// The first letter of the text that follows, with the punctuation around
    /// it, forms a box of its own. `first_line` supplies its first-line style,
    /// as in [`open_box`](Self::open_box); `None` uses `style`.
    ///
    /// The letter is selected as CSS Pseudo-Elements 4 specifies:
    /// - leading whitespace remains outside the box;
    /// - leading punctuation and intervening spaces are included;
    /// - the letter is one grapheme cluster, including digits and symbols;
    /// - trailing punctuation and preceding spaces are included, except
    ///   opening punctuation and dashes.
    ///
    /// Where a text node ends in punctuation, selection continues in the
    /// next node. The box holds only the part in the first node. Each part
    /// keeps its own `text-transform`; the letter takes the transform in
    /// `style`.
    ///
    /// Resolve `style` and `first_line` against the enclosing box. A call
    /// before a letter is found replaces the styles, so the method may be
    /// called before each text node.
    ///
    /// A block has at most one first letter. Calls are ignored once it is
    /// found, and after an atomic inline, a forced break, a ruby container,
    /// or text that can have no first letter.
    ///
    /// The letter's text runs carry `key`, for painting the pseudo-element.
    /// [`Layout::box_fragments`] returns its box.
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
    /// Closes every open container, then analyzes, selects fonts, shapes and
    /// measures the content with the builder's context.
    /// [`Layout::intrinsic_sizes`](crate::Layout::intrinsic_sizes) is
    /// available after this call.
    pub fn finish(self) -> BuildReport {
        self.finish_with_provider(None)
    }

    /// Prepares the layout with the host's glyph metrics.
    ///
    /// Pass the same provider, with the same strike settings, to
    /// [`Layout::break_lines_with_metrics`](crate::Layout::break_lines_with_metrics).
    pub fn finish_with_metrics(self, provider: &dyn FontMetricsProvider) -> BuildReport {
        self.finish_with_provider(Some(provider))
    }

    fn finish_with_provider(mut self, provider: Option<&dyn FontMetricsProvider>) -> BuildReport {
        let mut report = self.writer.finish();
        // `PreparedStages::prepare` in layout/mod.rs runs the preparation.
        let (content, cx) = self.writer.parts();
        self.stages.prepare(content, cx, provider, &mut report);
        self.prepared = true;
        report
    }
}

impl Drop for LayoutBuilder<'_> {
    /// Closes the content of a builder dropped without
    /// [`finish`](LayoutBuilder::finish), leaving a valid layout.
    ///
    /// The layout is not prepared: it holds no analysis, fonts or glyphs
    /// until it is built again.
    fn drop(&mut self) {
        if !self.prepared {
            self.writer.finish();
            self.stages.clear();
        }
    }
}
