//! The cluster writer: one pass over the items and the text that writes the
//! analysis.
//!
//! Start at: [`ClusterWriter::visit_item`], once per item in reading order,
//! then [`ClusterWriter::finish`]. A text-bearing item's characters go
//! through `ClusterWriter::text_item`, and each cluster through
//! `ClusterWriter::start_cluster` and `ClusterWriter::finish_cluster`.
//!
//! - **Each cluster is written once, whole,** when the next one starts. Only
//!   then are its end facts known: the break at the boundary, the next item
//!   and character, and any empty items between (box edges, floats, ruby
//!   marks) that stop shaping there.
//! - **A cluster starts** at every grapheme boundary, every text item's start
//!   and after every U+FFFC.
//! - **A paragraph ends** after a separator. Its line stream starts at its
//!   first cluster and resumes where a text item changes the break options.
//!   Its base level and flags are written when it ends.

use core::mem;

use super::breaks::{LineKey, LineStream};
use super::classify::is_east_asian;
use super::classify::{
    first_class, glues, hangs, is_latin1_fast, latin1_fast_char, latin1_fast_class,
};
use super::graphemes::GraphemeCursor;
use super::hosted::{self, Hosted, ItemWords};
use super::scripts::{self, RunBuilder, Setting};
use super::{
    Analysis, AnalysisContext, AnalysisInput, AnalysisScratch, BidiLevel, ClusterAttrs,
    ClusterClass, ClusterId, Paragraph, ParagraphFlags, ParagraphState, RunOrientation, bidi,
};
use crate::config::SmallKana;
use crate::data::{Id, TextOffset, define_flags};
use crate::stages::content::{
    BoxFacts, Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind, LanguageId, MAX_RUBY_DEPTH,
    NodeId, TextFlags, TextSetting,
};
use crate::style::{
    Direction, FirstLineVariant, Language, TextCombineUpright, WhiteSpaceCollapse, WordBreak,
};
use crate::unicode::{self, CoreProps, RareProps};
use crate::unit::LayoutUnit;
use crate::work;

/// U+FFFC OBJECT REPLACEMENT CHARACTER. A cluster always ends after one, so
/// a combining mark after an atomic inline does not join it.
const OBJECT: char = '\u{FFFC}';

/// The node and break-related style of the text-bearing item a cluster is
/// in.
#[derive(Copy, Clone, Debug)]
pub(super) struct ItemInfo {
    node: NodeId,
    pub(super) kind: ItemKind,
    key: LineKey,
    language: LanguageId,
    wraps: bool,
    collapse: WhiteSpaceCollapse,
    /// Spaces, tabs and other space separators ending a line hang
    /// (`Text::white_space_hangs`).
    pub(super) white_space_hangs: bool,
    hyphens_none: bool,
    /// Whether `overflow-wrap` is `anywhere` or `break-word` and the text
    /// wraps.
    emergency: bool,
    /// Whether the item is inside a ruby annotation.
    annotation: bool,
    /// Whether the item is a generated break opportunity, whose U+200B has a
    /// class of its own.
    pub(super) generated: bool,
    setting: TextSetting,
    /// What `text-combine-upright` combines; nothing outside a vertical line.
    combine: TextCombineUpright,
}

impl ItemInfo {
    /// Returns the facts of `item`, a text-bearing item of `content`.
    fn new(content: &Content, item: &Item) -> Self {
        Self {
            kind: item.kind,
            annotation: item.flags.contains(ItemFlags::ANNOTATION),
            generated: item.flags.contains(ItemFlags::GENERATED),
            ..Self::from_node(content, item.node)
        }
    }

    /// Returns the facts of the block's own style, which stand before the
    /// first item and for a content with no items.
    pub(super) fn from_block(content: &Content) -> Self {
        Self {
            emergency: false,
            ..Self::from_node(content, NodeId::BLOCK)
        }
    }

    /// Returns the facts of a text item of `node`.
    fn from_node(content: &Content, node: NodeId) -> Self {
        let facts = &content.facts;
        let id = content.nodes.text_facts(node, FirstLineVariant::Standard);
        let text = facts.text(id);
        let flag = |flag: TextFlags| text.has(flag);
        Self {
            node,
            kind: ItemKind::Text,
            key: LineKey::new(
                text.line_break,
                text.word_break,
                flag(TextFlags::CJK_LINE_BREAKS),
            ),
            language: facts.request(facts.text_request(id)).language,
            wraps: flag(TextFlags::WRAPS),
            collapse: text.collapse,
            white_space_hangs: flag(TextFlags::WHITE_SPACE_HANGS),
            hyphens_none: flag(TextFlags::HYPHENS_NONE),
            emergency: flag(TextFlags::EMERGENCY),
            annotation: false,
            generated: false,
            setting: text.setting,
            combine: text.combine,
        }
    }

    /// Returns whether the item sets the break options. Only text does: an
    /// atomic inline's U+FFFC and a `<br>`'s `\n` break under the options
    /// around them, so the stream does not resume at each.
    fn sets_key(&self) -> bool {
        self.kind == ItemKind::Text
    }
}

define_flags! {
    /// Emoji facts about a cluster's characters after its first.
    struct EmojiMarks(u8) {
        const VS15 = 1 << 0;
        const VS16 = 1 << 1;
        const KEYCAP = 1 << 2;
        const MODIFIER = 1 << 3;
        const ZWJ = 1 << 4;
        /// A ZWJ followed by a pictograph.
        const SEQUENCE = 1 << 5;
        const TAG = 1 << 6;
        /// A second regional indicator: a flag.
        const FLAG = 1 << 7;
    }
}

impl EmojiMarks {
    /// The marks that make a cluster an emoji whatever its first character:
    /// a keycap, a modifier, a ZWJ sequence, a tag or a flag.
    const EMOJI: Self =
        Self(Self::KEYCAP.0 | Self::MODIFIER.0 | Self::SEQUENCE.0 | Self::TAG.0 | Self::FLAG.0);

    /// Returns whether any flag in `other` is set.
    fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// The cluster being gathered, written when the next one starts.
#[derive(Copy, Clone, Debug)]
struct Pending {
    start: usize,
    item: ItemInfo,
    /// The class from the first character alone; see [`Pending::class()`].
    class: ClusterClass,
    /// The first character's core word.
    props: CoreProps,
    first: char,
    last: char,
    marks: EmojiMarks,
    /// Whether it holds a U+200C ZERO WIDTH NON-JOINER, where Blink ends a
    /// shaping run if white space is preserved.
    zwnj: bool,
    /// Whether a character after the first is a variation selector.
    selector: bool,
    /// Whether an empty item after it stops shaping at its end.
    shape_break: bool,
    /// Whether it is the later part of a grapheme split by an item boundary.
    continuation: bool,
    /// The ruby column, numbered in order from 1; 0 outside every column.
    /// Covers the column's base and annotations.
    column: u32,
    /// The annotation within the column, or 0 for the base. No inner wrap
    /// opportunity is shared across parts.
    part: u32,
    /// The orientation and combined unit (numbered from 1; 0 for none).
    setting: Setting,
}

impl Pending {
    fn new(
        start: usize,
        item: ItemInfo,
        ch: char,
        props: CoreProps,
        rare: Option<RareProps>,
        continuation: bool,
    ) -> Self {
        Self {
            start,
            item,
            class: first_class(ch, props, rare, &item),
            props,
            first: ch,
            last: ch,
            marks: EmojiMarks::default(),
            zwnj: ch == '\u{200C}',
            selector: false,
            shape_break: false,
            continuation,
            column: 0,
            part: 0,
            setting: Setting::HORIZONTAL,
        }
    }

    /// Adds `ch`, a character after the first, to the cluster.
    fn extend(&mut self, ch: char, props: CoreProps) {
        self.last = ch;
        let marks = &mut self.marks;
        match ch {
            '\u{FE0E}' => {
                marks.insert(EmojiMarks::VS15);
                self.selector = true;
            }
            '\u{FE0F}' => {
                marks.insert(EmojiMarks::VS16);
                self.selector = true;
            }
            '\u{FE00}'..='\u{FE0D}' | '\u{E0100}'..='\u{E01EF}' => self.selector = true,
            '\u{20E3}' => marks.insert(EmojiMarks::KEYCAP),
            '\u{200C}' => self.zwnj = true,
            '\u{200D}' => marks.insert(EmojiMarks::ZWJ),
            '\u{1F3FB}'..='\u{1F3FF}' => marks.insert(EmojiMarks::MODIFIER),
            '\u{E0020}'..='\u{E007F}' => marks.insert(EmojiMarks::TAG),
            _ if props.is_regional_indicator() => marks.insert(EmojiMarks::FLAG),
            _ if marks.contains(EmojiMarks::ZWJ) && props.is_extended_pictographic() => {
                marks.insert(EmojiMarks::SEQUENCE);
            }
            _ => {}
        }
    }

    /// Returns the class, adjusted for the characters after the first.
    ///
    /// A keycap makes a digit an emoji. VS15 makes an emoji a symbol. VS16, a
    /// modifier, a tag or a ZWJ sequence makes a symbol an emoji.
    fn class(&self) -> ClusterClass {
        let marks = self.marks;
        match self.class {
            ClusterClass::Text if marks.contains(EmojiMarks::KEYCAP) => ClusterClass::Emoji,
            ClusterClass::Emoji
                if marks.contains(EmojiMarks::VS15) && !marks.intersects(EmojiMarks::EMOJI) =>
            {
                ClusterClass::Symbol
            }
            ClusterClass::Symbol if marks.intersects(EmojiMarks::EMOJI.union(EmojiMarks::VS16)) => {
                ClusterClass::Emoji
            }
            class => class,
        }
    }
}

/// The last cluster of a ruby base, which a break after the base reads.
#[derive(Copy, Clone, Debug)]
struct BaseTail {
    item: ItemInfo,
    last: char,
    class: ClusterClass,
    end: TextOffset,
}
impl BaseTail {
    fn new(pending: Pending, end: usize) -> Self {
        Self {
            item: pending.item,
            last: pending.last,
            class: pending.class(),
            end: TextOffset::new(end),
        }
    }
}

/// A ruby base being gathered, which decides a break after it.
#[derive(Copy, Clone, Debug, Default)]
struct RubyBase {
    /// The base's start and the item it starts in.
    start: Option<(TextOffset, ItemId)>,
    /// The last cluster before an annotation or the container's close.
    last: Option<BaseTail>,
    /// Whether an annotation ended since the base's text, so base text
    /// after it starts the base again.
    after_annotation: bool,
    /// Whether an annotation of the container opens after its last opening
    /// or annotation-closing item, before the container closes.
    annotation_follows: bool,
}

impl RubyBase {
    /// Returns a base starting at `at`, in item `item`.
    fn new(at: TextOffset, item: ItemId) -> Self {
        Self {
            start: Some((at, item)),
            ..Self::default()
        }
    }

    /// Ends the base with `pending` at `end`, if `pending` is base text that
    /// starts within the base.
    fn end_with(&mut self, pending: Option<Pending>, end: TextOffset) {
        if let Some(pending) = pending
            && !pending.item.annotation
            && self
                .start
                .is_some_and(|(start, _)| pending.start >= start.get())
        {
            self.last = Some(BaseTail::new(pending, end.get()));
        }
    }
}

/// The rule that decides a line break between two clusters
/// ([`ClusterWriter::boundary`]).
#[derive(Copy, Clone, Debug)]
enum Boundary {
    /// Line text, decided by the paragraph's line stream.
    Text,
    /// Inside one part of a ruby column (its base or an annotation), decided
    /// by the part's own line stream.
    InsidePart,
    /// After a ruby column, decided by its base and the text after it.
    AfterColumn,
    /// After a nested container's base inside its parent's base, decided by
    /// that base and the text after it.
    AfterNestedBase(RubyBase),
    /// Between two parts of one column, or inside a combined unit: no break.
    Held,
}

/// The writer's position among the ruby columns, root and nested.
///
/// A stack holds the base being gathered in each open container. Clusters
/// update only the innermost base; a closing nested container hands its
/// base's last cluster to its parent.
#[derive(Copy, Clone, Debug, Default)]
struct RubyState {
    /// The number of containers open as columns. A root container is a
    /// column only if an annotation follows it.
    containers: u32,
    /// The base being gathered in each open container, outermost first.
    bases: [RubyBase; MAX_RUBY_DEPTH],
    /// The base of a nested container that ended since the last cluster
    /// started; it decides the next boundary
    /// ([`Boundary::AfterNestedBase`]).
    inner_boundary: Option<RubyBase>,
    /// The number of open annotations.
    annotations: u32,
    /// The number of the last annotation opened, its clusters' part.
    part: u32,
    /// The number given to the last root column.
    serial: u32,
    /// The root column of the next cluster, or 0 outside every column.
    column: u32,
    /// The column of the last cluster started.
    seen: u32,
}

impl RubyState {
    /// Returns the innermost open container's base, if any.
    fn innermost(&mut self) -> Option<&mut RubyBase> {
        let depth = self.containers.checked_sub(1)?;
        self.bases.get_mut(depth as usize)
    }

    /// Opens a container as a column, at `at` in item `item`.
    ///
    /// A root container starts the next column but keeps the previous
    /// column's base: the new column's first cluster still needs it to
    /// decide the boundary, then resets it
    /// ([`enter_column`](Self::enter_column)). `annotation_follows` is true
    /// if an annotation of the container follows.
    fn open(&mut self, at: TextOffset, item: ItemId, annotation_follows: bool) {
        self.containers = self.containers.saturating_add(1);
        if self.containers == 1 {
            self.serial = self.serial.saturating_add(1);
            self.column = self.serial;
            self.bases[0].after_annotation = false;
        } else if let Some(base) = self.innermost() {
            *base = RubyBase::new(at, item);
        }
        if let Some(base) = self.innermost() {
            base.annotation_follows = annotation_follows;
        }
    }

    /// Closes the innermost container at `at`, after the cluster `pending`.
    ///
    /// Its base ends. A nested base hands its last cluster to its parent and
    /// decides the next boundary.
    fn close(&mut self, pending: Option<Pending>, at: TextOffset) {
        if let Some(depth) = self.containers.checked_sub(1).map(|depth| depth as usize)
            && let Some(base) = self.bases.get_mut(depth)
        {
            base.end_with(pending, at);
            let base = *base;
            if let Some(parent) = depth
                .checked_sub(1)
                .and_then(|parent| self.bases.get_mut(parent))
            {
                self.inner_boundary = Some(base);
                if base.last.is_some() {
                    parent.last = base.last;
                }
            }
        }
        self.containers = self.containers.saturating_sub(1);
        if self.containers == 0 {
            self.column = 0;
            self.annotations = 0;
            self.bases[0].after_annotation = false;
        }
    }

    /// Opens an annotation at `at`, ending the innermost base with the
    /// cluster `pending`.
    fn annotation_open(&mut self, pending: Option<Pending>, at: TextOffset) {
        if let Some(base) = self.innermost() {
            base.end_with(pending, at);
        }
        self.annotations = self.annotations.saturating_add(1);
        self.part = self.part.saturating_add(1);
    }

    /// Starts the innermost container's next column over an empty base, at
    /// an annotation that opened.
    ///
    /// A root column takes the next number, and no line breaks after it, as
    /// after Chrome's empty base. A nested base starts again, empty.
    fn empty_base(&mut self) {
        if self.containers == 1 {
            self.serial = self.serial.saturating_add(1);
            self.column = self.serial;
            self.bases[0].after_annotation = false;
        } else if let Some(base) = self.innermost() {
            *base = RubyBase {
                annotation_follows: base.annotation_follows,
                ..RubyBase::default()
            };
        }
    }

    /// Closes an annotation. Once none is open, base text after it starts
    /// the base again, and a nested base decides the next boundary.
    /// `annotation_follows` is true if another annotation of the container
    /// follows.
    fn annotation_close(&mut self, annotation_follows: bool) {
        self.annotations = self.annotations.saturating_sub(1);
        if let Some(base) = self.innermost() {
            base.annotation_follows = annotation_follows;
        }
        let nested = self.containers > 1;
        if self.annotations == 0
            && let Some(base) = self.innermost()
        {
            let ended = *base;
            base.after_annotation = true;
            if nested {
                self.inner_boundary = Some(ended);
            }
        }
    }

    /// Enters a cluster starting at `at` in item `item` and `column`.
    /// `first` is true for the column's first cluster, `annotation` for an
    /// annotation's.
    ///
    /// - **The root base** resets at a column's first cluster, once the
    ///   break after the previous column is decided. It starts at the
    ///   column's first base cluster.
    /// - **A nested base** starts at its container's open and restarts at
    ///   its first base cluster after an annotation.
    fn enter_column(
        &mut self,
        column: u32,
        first: bool,
        at: TextOffset,
        item: ItemId,
        annotation: bool,
    ) {
        if first {
            self.seen = column;
            self.bases[0] = RubyBase::default();
        }
        self.inner_boundary = None;
        if annotation {
            return;
        }
        if column != 0 && self.bases[0].start.is_none() {
            self.bases[0].start = Some((at, item));
        }
        if self.containers > 1
            && let Some(base) = self.innermost()
            && base.after_annotation
        {
            *base = RubyBase {
                annotation_follows: base.annotation_follows,
                ..RubyBase::new(at, item)
            };
        }
    }
}

/// The writer's position among the units `text-combine-upright` combines.
///
/// Each unit is set as one character and one run, and no line breaks inside
/// it.
#[derive(Copy, Clone, Debug, Default)]
struct CombineState {
    /// The number given to the last unit.
    serial: u16,
    /// Under `all`, the unit the next cluster joins, or 0 if it starts one.
    /// A unit covers one text item and the text items straight after it, as
    /// Blink combines a text node and its text siblings in one
    /// `LayoutTextCombine`.
    all: u16,
    /// Under `digits`, the open unit and how many of its clusters remain.
    digits: Option<(u16, usize)>,
}

impl CombineState {
    /// Returns the number for the next unit.
    ///
    /// It skips 0, which names no unit, on wrapping, so it never repeats the
    /// unit before (`Setting::unit`).
    fn next_unit(&mut self) -> u16 {
        self.serial = self.serial.checked_add(1).unwrap_or(1);
        self.serial
    }
}

/// The number of characters after a ruby column read to decide a break
/// after it.
///
/// UAX #14 reads at most a few characters past a boundary (LB25's numbers,
/// LB28a's aksaras). The bound keeps the cost per column constant however
/// long the paragraph.
const COLUMN_LOOKAHEAD: usize = 8;

/// The writer's state over one content.
pub(super) struct ClusterWriter<'a> {
    content: &'a Content,
    text: &'a str,
    /// The text as the line segmenter reads it under the Japanese and Chinese
    /// options: `text` with its curly double quotes swapped, or `text`.
    cj_text: &'a str,
    cx: &'a AnalysisContext,
    scratch: &'a mut AnalysisScratch,
    out: &'a mut Analysis,
    graphemes: GraphemeCursor<'a>,
    /// The paragraph's line stream.
    stream: Option<LineStream<'a>>,
    /// The current ruby part's line stream, over text that ends before the
    /// next part.
    inner_stream: Option<LineStream<'a>>,
    inner_start: usize,
    pending: Option<Pending>,
    /// The text-bearing item the characters come from.
    item: ItemInfo,
    /// The item being walked.
    item_id: ItemId,
    /// The byte where the text item being walked starts.
    item_start: usize,
    /// What the host's segmenters change in the opportunities of the text
    /// item being walked ([`ClusterWriter::begin_hosted`]).
    hosted: Hosted,
    /// Some style asks for `hyphens: auto` or `auto-phrase`, and the context
    /// has a hyphenator or a phrase segmenter. Without both, no item asks the
    /// host.
    asks_host: bool,
    /// The byte where the text item being walked ends.
    item_end: usize,
    /// Whether the last character was U+FFFC.
    after_object: bool,
    paragraph: ParagraphState,
    script_runs: RunBuilder,
    /// Whether a box with `unicode-bidi` or a ruby annotation opened or
    /// closed since the last cluster, giving the next cluster's paragraph
    /// controls. The bidi input tracks which are open, and a paragraph
    /// starting inside one after a forced break reopens them.
    controls: bool,
    /// Whether the block overrides bidi, which every paragraph starts with.
    block_overrides: bool,
    /// The paragraph level the block's direction sets, or `None` under
    /// `auto`, where each paragraph's first strong character decides.
    requested: Option<BidiLevel>,
    /// Whether some style is `nowrap`, so a break between two nodes must
    /// find whose style decides it.
    any_nowrap: bool,
    /// Whether the content has atomic inlines, whose edges have their own
    /// opportunity rule.
    any_atomics: bool,
    /// Whether a small kana is held to what precedes it under
    /// `line-break: normal` and `auto`, as CSS Text says, instead of starting a line as
    /// in Chrome (`Config::small_kana`).
    hold_small_kana: bool,
    ruby: RubyState,
    combine: CombineState,
}

impl<'a> ClusterWriter<'a> {
    /// Starts the writer over the content of `input`, writing into `out`.
    ///
    /// `cj_text` is the content's text with its curly double quotes swapped
    /// (`breaks::fill_cj_text`), or `None` where it has none.
    pub(super) fn new(
        input: &AnalysisInput<'a>,
        cx: &'a AnalysisContext,
        scratch: &'a mut AnalysisScratch,
        out: &'a mut Analysis,
        cj_text: Option<&'a str>,
    ) -> Self {
        let AnalysisInput {
            content,
            small_kana,
        } = *input;
        let text = content.text.as_str();
        let flags = content.flags;
        // Stands until the first text-bearing item, before any cluster.
        let item = ItemInfo::from_block(content);
        Self {
            content,
            text,
            cj_text: cj_text.unwrap_or(text),
            cx,
            scratch,
            out,
            graphemes: GraphemeCursor::new(text, cx.graphemes()),
            stream: None,
            inner_stream: None,
            inner_start: 0,
            pending: None,
            item,
            item_id: ItemId::default(),
            item_start: 0,
            hosted: Hosted::None,
            asks_host: content.flags.contains(ContentFlags::HOSTED_BREAKS)
                && (cx.hyphenator.is_some() || cx.phrases.is_some()),
            item_end: 0,
            after_object: false,
            paragraph: ParagraphState::default(),
            script_runs: RunBuilder::new(),
            controls: false,
            // A left-to-right override only raises even levels to even ones,
            // which Blink leaves at 0 (`bidi::controls`).
            block_overrides: content.block.overrides == Some(Direction::Rtl),
            requested: BidiLevel::from_direction(content.block.direction),
            any_nowrap: flags.contains(ContentFlags::NOWRAP),
            any_atomics: flags.contains(ContentFlags::ATOMICS),
            hold_small_kana: small_kana == SmallKana::Held,
            ruby: RubyState::default(),
            combine: CombineState::default(),
        }
    }

    /// Updates the bidi input's open containers at `item`, the edge of a
    /// box, ruby container or annotation, and returns its node's box facts.
    fn bidi_edge(&mut self, item: &Item) -> &'a BoxFacts {
        let content = self.content;
        let id = content
            .nodes
            .box_facts(item.node, FirstLineVariant::Standard);
        let facts = content.facts.box_facts(id);
        self.controls |= self.scratch.bidi.walk(content, item, facts);
        facts
    }
}

impl ClusterWriter<'_> {
    /// Returns the id of the next cluster to start, counting the pending
    /// one, which is not yet written.
    fn next_cluster(&self) -> ClusterId {
        ClusterId::new(self.out.clusters.len() + usize::from(self.pending.is_some()))
    }

    /// Visits item `id`: records its first cluster, then walks its text or
    /// applies its effect on the clusters beside it.
    pub(super) fn visit_item(&mut self, id: ItemId, item: &Item) {
        self.item_id = id;
        let first = self.next_cluster();
        if self.out.item_clusters.push(first).is_none() {
            debug_assert!(false, "one entry per item, which an ItemId names");
        }
        // Any item other than text ends a combined unit.
        if item.kind != ItemKind::Text {
            self.combine.all = 0;
            self.combine.digits = None;
        }
        match item.kind {
            ItemKind::Text | ItemKind::Atomic | ItemKind::Break => self.text_item(item),
            ItemKind::Open | ItemKind::Close => self.edge(item),
            // An annotation is an isolate, so it adds controls like a box's
            // `unicode-bidi`.
            ItemKind::AnnotationOpen => {
                self.bidi_edge(item);
                self.ruby.annotation_open(self.pending, item.start);
                if item.flags.contains(ItemFlags::EMPTY_BASE) {
                    self.ruby.empty_base();
                }
                self.stop_shaping();
            }
            ItemKind::AnnotationClose => {
                self.bidi_edge(item);
                self.ruby
                    .annotation_close(item.flags.contains(ItemFlags::ANNOTATION_FOLLOWS));
                self.stop_shaping();
            }
            ItemKind::RubyOpen => {
                // The container's `unicode-bidi` adds controls like a box's.
                self.bidi_edge(item);
                self.stop_shaping();
                // A root container with no annotation is not a column: its
                // text is line text, like a box's. Its close then finds no
                // container open.
                let follows = item.flags.contains(ItemFlags::ANNOTATION_FOLLOWS);
                if self.ruby.containers == 0 && !follows {
                    return;
                }
                // A column starts with the container.
                self.ruby.open(item.start, id, follows);
            }
            ItemKind::RubyClose => {
                self.bidi_edge(item);
                self.ruby.close(self.pending, item.start);
                self.stop_shaping();
            }
            ItemKind::Float | ItemKind::Absolute => self.stop_shaping(),
        }
    }

    /// Returns the ruby column of a cluster starting now, and whether it is
    /// the column's first.
    ///
    /// Base text after an annotation starts the container's next column if
    /// an annotation follows it, and is line text otherwise.
    fn next_column(&mut self) -> (u32, bool) {
        let ruby = &mut self.ruby;
        if ruby.containers == 0 {
            return (0, false);
        }
        if !self.item.annotation && ruby.bases[0].after_annotation {
            ruby.bases[0].after_annotation = false;
            if ruby.innermost().is_some_and(|base| base.annotation_follows) {
                ruby.serial = ruby.serial.saturating_add(1);
                ruby.column = ruby.serial;
            } else {
                ruby.column = 0;
            }
        }
        (ruby.column, ruby.column != ruby.seen)
    }

    /// Handles a box's opening or closing edge, stopping shaping where
    /// `BoxFacts::breaks_shaping` says so.
    fn edge(&mut self, item: &Item) {
        let facts = self.bidi_edge(item);
        let opens = item.kind == ItemKind::Open;
        if facts.breaks_shaping(opens) {
            self.stop_shaping();
        }
    }

    /// Marks a shaping stop on the pending cluster's end.
    ///
    /// A separator stops shaping anyway, and before the first cluster there
    /// is nothing to mark. `HAS_SHAPING_STOPS` stays unset, since shaping's
    /// fast path already stops at item edges.
    fn stop_shaping(&mut self) {
        if let Some(pending) = &mut self.pending
            && !pending.class.is_forced_break()
        {
            pending.shape_break = true;
        }
    }

    /// Walks the characters of a text item, an atomic's U+FFFC or a `<br>`'s
    /// `\n`.
    fn text_item(&mut self, item: &Item) {
        self.item = ItemInfo::new(self.content, item);
        if self.item.combine != TextCombineUpright::All {
            self.combine.all = 0;
        }
        let start = item.start.get();
        self.item_start = start;
        self.item_end = item.end.get();
        let Some(text) = self.text.get(start..item.end.get()) else {
            debug_assert!(false, "items fall on character boundaries");
            return;
        };
        if self.asks_host {
            self.begin_hosted();
        }
        // An atomic inline is its own cluster on both sides, so it is never
        // a continuation of what precedes it.
        let object = self.item.kind == ItemKind::Atomic;
        // The Latin-1 fast path runs only in horizontal text outside ruby,
        // where the host's segmenters change nothing.
        let fast_path = work::fast_paths()
            && self.item.kind == ItemKind::Text
            && self.item.setting == TextSetting::Horizontal
            && self.ruby.containers == 0
            && self.hosted == Hosted::None;
        let mut chars = text.char_indices();
        // The offset of `chars` in the item's text, which the fast path
        // advances.
        let mut base = 0;
        while let Some((offset, ch)) = chars.next() {
            let offset = base + offset;
            let at = start + offset;
            let props = unicode::core_props(ch);
            // The rare word is read once per character past Latin-1, for the
            // grapheme step and possibly the class and orientation. Latin-1
            // needs none.
            let rare = (ch >= '\u{100}').then(|| unicode::rare_props(ch));
            let grapheme = self.graphemes.boundary(at, ch, rare);
            // Marks after a lone space start their own cluster. UAX #14 does
            // not attach marks to a space (LB9) and allows a break between
            // them (LB10, LB18), as Chrome and Firefox do, inside the
            // grapheme. Carets keep to the grapheme
            // (`Clusters::continues_grapheme`).
            let after_space = !grapheme
                && self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.last == ' ' && pending.start + 1 == at);
            let starts = offset == 0 || grapheme || self.after_object || after_space;
            // A cluster that starts only because an item does continues a
            // grapheme split by the item boundary.
            let continuation = !grapheme && !self.after_object && !object && !after_space;
            if starts {
                self.start_cluster(at, ch, props, rare, continuation);
            } else if let Some(pending) = &mut self.pending {
                pending.extend(ch, props);
            }
            let paragraph = &mut self.paragraph;
            paragraph.classes |= props.bidi_class().mask();
            paragraph.east_asian |= is_east_asian(ch);
            self.after_object = ch == OBJECT;
            // After a fast-path character that started its own cluster, the
            // fast path takes the following fast-path characters, and this
            // loop resumes where it stops.
            let from = offset + ch.len_utf8();
            if fast_path
                && starts
                && !continuation
                && is_latin1_fast(ch)
                && let Some(next) = latin1_fast_char(text.as_bytes(), from)
            {
                base = self.latin1_fast_path(text, start, from, next);
                chars = text.get(base..).unwrap_or_default().char_indices();
            }
        }
    }

    /// Asks the host's segmenters about the text item being walked, and sets
    /// what its opportunities take from them.
    ///
    /// Phrases need a phrase segmenter, which is asked only about Japanese
    /// text; without one, `auto-phrase` text takes hyphens as other text
    /// does. Hyphens need a hyphenator and a language. Ruby and annotations
    /// take neither. Out of line: few texts ask.
    #[inline(never)]
    fn begin_hosted(&mut self) {
        self.hosted = Hosted::None;
        self.scratch.hosted.clear();
        let item = &self.item;
        if item.kind != ItemKind::Text
            || !item.wraps
            || item.annotation
            || self.ruby.containers != 0
        {
            return;
        }
        let content = self.content;
        let cx = self.cx;
        let id = content
            .nodes
            .text_facts(item.node, FirstLineVariant::Standard);
        let text = content.facts.text(id);
        let language = content.languages.get(item.language);
        let range = TextOffset::new(self.item_start)..TextOffset::new(self.item_end);
        if text.word_break == WordBreak::AutoPhrase
            && let Some(segmenter) = cx.phrases.as_deref()
        {
            if language.language().eq_ignore_ascii_case("ja") {
                self.hosted = Hosted::Phrases;
                let points = &mut self.scratch.hosted;
                hosted::segment_phrases(points, self.text, range, language, segmenter);
            } else {
                self.hosted = Hosted::Unsegmented;
            }
            return;
        }
        if text.has(TextFlags::HYPHENS_AUTO)
            && language != Language::UND
            && let Some(hyphenator) = cx.hyphenator.as_deref()
        {
            self.hosted = Hosted::Hyphens;
            let key = self.item.key;
            let words = ItemWords {
                text: self.text,
                cj_text: self.cj_text,
                range,
                key,
                segmenter: cx.line(key),
                language,
            };
            hosted::hyphenate(&mut self.scratch.hosted, &words, hyphenator);
        }
    }

    /// Writes clusters for the fast-path characters ([`latin1_fast_char`])
    /// from byte `from` of `text`, and returns the offset in `text` where it
    /// stops.
    ///
    /// `text` is the item being walked, starting at byte `start` of the whole
    /// text, and `first` is the character at `from`. The pending cluster is a
    /// fast-path character that started its own cluster in this item.
    ///
    /// Each cluster is one character of its [`latin1_fast_class`], with no
    /// marks; two fast-path characters always have a grapheme boundary
    /// between them. The item is horizontal text outside ruby, the paragraph
    /// is open, its stream uses the item's key, and no control is waiting.
    /// So each cluster is written as [`finish_cluster`](Self::finish_cluster)
    /// would, without the parts that never apply here:
    /// - the ruby column, the combined unit and the stream's resume;
    /// - a ZWNJ, and a stop from an item between;
    /// - the held small kana (no Latin-1 character is one);
    /// - an atomic's edges and a separator;
    /// - the paragraph flags other than a soft hyphen's, and the East Asian
    ///   flag.
    ///
    /// The runs are consulted only where the cluster changes them
    /// ([`RunBuilder::joins`]). The last character is left pending, as in the
    /// general walk, so a following mark joins it and the grapheme cursor
    /// resumes from it.
    #[inline(never)]
    fn latin1_fast_path(
        &mut self,
        text: &str,
        start: usize,
        from: usize,
        first: (char, usize),
    ) -> usize {
        let bytes = text.as_bytes();
        let Some(pending) = self.pending else {
            return from;
        };
        let (mut ch, mut len) = first;
        let item = self.item;
        let key = item.key;
        let break_spaces = item.collapse == WhiteSpaceCollapse::BreakSpaces;
        let space_hangs = item.white_space_hangs && !item.annotation;
        let whole = self.text;
        let (mut last, mut last_class, mut last_props) =
            (pending.first, pending.class, pending.props);
        let mut last_start;
        let mut offset = from;
        loop {
            let at = start + offset;
            let props = unicode::core_props(ch);
            // The character before `last`, which the hyphen rule reads.
            let before_last = || {
                whole
                    .get(..at)
                    .and_then(|before| before.chars().rev().nth(1))
            };
            let stream = &mut self.stream;
            let mut breaks = key.ascii_or(before_last, last, ch, || {
                stream.as_mut().is_some_and(|stream| stream.at(at))
            });
            let space = last_class == ClusterClass::Space;
            breaks |= break_spaces && space;
            // Inside one item, the item's own wrapping decides, after a space
            // as elsewhere; this matches `filter_break` for one node.
            breaks &= item.wraps;
            let soft_hyphen = last_class == ClusterClass::SoftHyphen;
            breaks &= !(soft_hyphen && item.hyphens_none);
            let attrs = ClusterAttrs::new(last_class)
                .with(ClusterAttrs::BREAK_AFTER, breaks)
                .with(ClusterAttrs::EMERGENCY_AFTER, item.emergency)
                .with(ClusterAttrs::HANGS, space_hangs && space);
            let Some(id) = self
                .out
                .clusters
                .push(TextOffset::new(at), false, false, attrs)
            else {
                debug_assert!(
                    false,
                    "no more clusters than bytes, which a ClusterId names"
                );
                return offset;
            };
            if soft_hyphen {
                self.write_soft_hyphen(item.node, id, breaks);
            }
            let opens = id == self.paragraph.start;
            if opens || !self.script_runs.joins(last_props, item.language) {
                let started = self.script_runs.cluster(
                    &mut self.scratch.pending_runs,
                    id,
                    last_props,
                    item.language,
                    Setting::HORIZONTAL,
                    opens,
                );
                if let Some(orientation) = started {
                    self.paragraph
                        .flags
                        .insert(ParagraphFlags::from(orientation));
                }
            }
            (last, last_class, last_props, last_start) = (ch, latin1_fast_class(ch), props, at);
            offset += len;
            match latin1_fast_char(bytes, offset) {
                Some((next, next_len)) => (ch, len) = (next, next_len),
                None => break,
            }
        }
        // Leaves the last character pending, as the general walk does.
        self.pending = Some(Pending::new(
            last_start, item, last, last_props, None, false,
        ));
        self.graphemes.skip_to(last_start, last);
        offset
    }

    /// Flags the paragraph as holding a soft hyphen, cluster `id` of `node`,
    /// and records the node if a line may break after it.
    ///
    /// Out of line, so the fast path's loop pays one test for it.
    #[inline(never)]
    fn write_soft_hyphen(&mut self, node: NodeId, id: ClusterId, breaks: bool) {
        self.paragraph.flags.insert(ParagraphFlags::HAS_HYPHENS);
        if breaks {
            self.scratch.push_hyphen_node(node, id);
        }
    }

    /// Returns a cluster's orientation and combined unit, and advances the
    /// combined-unit state.
    ///
    /// The cluster starts at byte `at` with `first`, of class `class`, with
    /// the words `props` and `rare`. `continuation` is true if an item
    /// boundary split its grapheme.
    ///
    /// - **A continuation** takes the setting of the part before, since a
    ///   grapheme is set as one.
    /// - **Under `all`**, a forced break belongs to no unit, and the text
    ///   after it starts another.
    /// - **Under `digits`**, a unit is a maximal run of ASCII digits within
    ///   the item, no longer than the style allows. A longer run, or one
    ///   split by an item boundary, is set as ordinary digits.
    ///
    /// Called only for text in a vertical line. It takes values instead of
    /// the pending cluster so the cluster loop keeps that in registers.
    #[inline(never)]
    fn setting(
        &mut self,
        at: usize,
        first: char,
        class: ClusterClass,
        props: CoreProps,
        rare: Option<RareProps>,
        continuation: bool,
    ) -> Setting {
        if continuation && let Some(before) = &self.pending {
            return before.setting;
        }
        let item = &self.item;
        let (setting, combine) = (item.setting, item.combine);
        let mut unit = 0;
        match combine {
            TextCombineUpright::All if class.is_forced_break() => {
                self.combine.all = 0;
            }
            TextCombineUpright::All => {
                if self.combine.all == 0 {
                    self.combine.all = self.combine.next_unit();
                }
                unit = self.combine.all;
            }
            TextCombineUpright::Digits(most) if first.is_ascii_digit() => {
                unit = self.digits(at, usize::from(most));
            }
            TextCombineUpright::Digits(_) | TextCombineUpright::None => {
                self.combine.digits = None;
            }
        }
        let orientation = if unit == 0 {
            setting.orientation(first, props, rare)
        } else {
            RunOrientation::Combined
        };
        let follows = unit == 0 && setting.follows_script_run(props);
        Setting {
            orientation,
            unit,
            follows,
        }
    }

    /// Returns the combined unit of an ASCII digit at byte `at`, where the
    /// style combines runs of at most `most` digits.
    ///
    /// Returns the open unit, a new unit if a run starts here, or 0.
    fn digits(&mut self, at: usize, most: usize) -> u16 {
        if let Some((unit, left)) = self.combine.digits
            && left > 0
        {
            self.combine.digits = Some((unit, left - 1));
            return unit;
        }
        self.combine.digits = None;
        // A digit after a digit is inside a run too long to combine.
        let before = self
            .text
            .get(..at)
            .and_then(|text| text.chars().next_back());
        if before.is_some_and(|ch| ch.is_ascii_digit()) {
            return 0;
        }
        // Counts one past the limit to detect a run too long.
        let rest = self.text.get(at..).unwrap_or_default().as_bytes();
        let run = rest
            .iter()
            .take(most + 1)
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let fits = at.saturating_add(run) <= self.item_end;
        if run == 0 || run > most || !fits {
            return 0;
        }
        let unit = self.combine.next_unit();
        self.combine.digits = Some((unit, run - 1));
        unit
    }

    /// Starts a cluster at `at` with `ch`, and writes the one before.
    ///
    /// `props` and `rare` are `ch`'s words; `rare` is `None` for Latin-1.
    fn start_cluster(
        &mut self,
        at: usize,
        ch: char,
        props: CoreProps,
        rare: Option<RareProps>,
        continuation: bool,
    ) {
        let mut next = Pending::new(at, self.item, ch, props, rare, continuation);
        let (column, first_in_column) = self.next_column();
        next.column = column;
        if self.item.annotation {
            next.part = self.ruby.part;
        }
        // Horizontal text has one orientation and no unit; testing here
        // costs nearly every layout one branch per cluster.
        if self.item.setting != TextSetting::Horizontal {
            next.setting = self.setting(at, ch, next.class, props, rare, continuation);
        }
        let starts_part = column != 0
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.column != column || pending.part != next.part);
        if let Some(pending) = self.pending.take() {
            // The options after a seam decide the pair at it, so the stream
            // resumes before the boundary is queried.
            if self.item.sets_key() {
                if !pending.class.is_forced_break() {
                    self.resume_stream(false, at);
                }
                if pending.column != 0 && pending.column == next.column && pending.part == next.part
                {
                    self.resume_stream(true, at);
                }
            }
            self.finish_cluster(pending, Some(&next), at);
        }
        self.ruby.enter_column(
            column,
            first_in_column,
            TextOffset::new(at),
            self.item_id,
            next.item.annotation,
        );
        if !self.paragraph.open {
            self.open_paragraph();
            let key = self.item.key;
            self.stream = Some(LineStream::new(
                self.text,
                self.cj_text,
                at,
                self.cx.line(key),
                key,
            ));
        }
        self.paragraph.controls |= mem::take(&mut self.controls);
        if starts_part {
            self.begin_inner_part(at);
        } else if column == 0 {
            self.inner_stream = None;
        }
        self.pending = Some(next);
    }

    /// Resumes the paragraph's line stream, or the ruby part's if `inner`,
    /// under the item's options from the seam `at`, if they differ.
    fn resume_stream(&mut self, inner: bool, at: usize) {
        let key = self.item.key;
        let stream = if inner {
            &mut self.inner_stream
        } else {
            &mut self.stream
        };
        if let Some(stream) = stream
            && stream.key() != key
        {
            stream.resume(self.cx.line(key), key, at);
        }
    }

    /// Opens a paragraph at the next cluster, in the item being walked.
    ///
    /// The paragraph has controls if boxes open across its start reopen
    /// them, or if the block overrides bidi.
    ///
    /// Out of line, like `end_paragraph` and `breaks_after_base`. Each runs
    /// once per paragraph or column, and inlining them slows the cluster loop
    /// by 2 to 3%.
    #[inline(never)]
    fn open_paragraph(&mut self) {
        let reopens = self.scratch.bidi.begin_paragraph(self.content);
        self.paragraph = ParagraphState {
            open: true,
            start: self.out.clusters.end_id(),
            first_item: self.item_id,
            controls: reopens || self.block_overrides,
            ..ParagraphState::default()
        };
    }

    /// Writes `pending`, which ends at `end`, where `next` starts or at the
    /// text's end if `next` is `None`.
    fn finish_cluster(&mut self, pending: Pending, next: Option<&Pending>, end: usize) {
        let class = pending.class();
        let mut attrs = ClusterAttrs::new(class);
        // Whether a break after the cluster draws a hyphen that its class
        // does not imply.
        let mut hyphen = false;
        // No opportunity at a paragraph's end, or before a separator, which
        // ends the line anyway. A filter that adds opportunities
        // (`break-spaces`, an atomic inline's) could otherwise leave the
        // separator on a line of its own.
        if let Some(next) = next
            && !class.is_forced_break()
            && !next.class.is_forced_break()
        {
            // An emergency break between two items is allowed if either
            // allows one, as Chrome's retry breaks before the first character
            // of text that allows `overflow-wrap`.
            let either = pending.item.emergency || next.item.emergency;
            let (breaks, emergency) = match self.boundary(&pending, next) {
                Boundary::Text if self.hosted != Hosted::None => {
                    let breaks = self.stream_breaks(false, &pending, class, next);
                    let hosted = self.hosted_break(&pending, class, next, breaks);
                    hyphen = hosted.2;
                    (hosted.0, either || hosted.1)
                }
                Boundary::Text => (self.stream_breaks(false, &pending, class, next), either),
                Boundary::InsidePart => (self.stream_breaks(true, &pending, class, next), either),
                Boundary::AfterColumn => (self.breaks_after_base(next, self.ruby.bases[0]), either),
                Boundary::AfterNestedBase(base) => (
                    self.breaks_after_base(next, base),
                    base.last.is_some_and(|last| last.item.emergency),
                ),
                Boundary::Held => (false, false),
            };
            attrs = attrs.with(ClusterAttrs::BREAK_AFTER, breaks).with(
                ClusterAttrs::EMERGENCY_AFTER,
                emergency && !next.continuation,
            );
        }
        // A ZWNJ stops shaping only where white space is preserved, as Blink
        // makes it a control item only under `preserve` and `break-spaces`.
        // Elsewhere the shaper kerns past it and ligates up to it. The stop
        // goes at its cluster's end; a kern or ligature cannot tell the side.
        let zwnj_stops = pending.zwnj && pending.item.collapse.keeps_spaces();
        if zwnj_stops {
            self.paragraph
                .flags
                .insert(ParagraphFlags::HAS_SHAPING_STOPS);
        }
        attrs = attrs
            // An annotation's white space is never at the end of the base's
            // line, and a combined unit's is inside its one character.
            .with(
                ClusterAttrs::HANGS,
                hangs(class, &pending.item)
                    && !pending.item.annotation
                    && pending.setting.unit == 0,
            )
            .with(
                ClusterAttrs::SHAPE_BREAK_AFTER,
                pending.shape_break || zwnj_stops,
            );
        let Some(id) = self.out.clusters.push(
            TextOffset::new(end),
            pending.continuation,
            pending.selector,
            attrs,
        ) else {
            debug_assert!(
                false,
                "no more clusters than bytes, which a ClusterId names"
            );
            return;
        };
        if class == ClusterClass::SoftHyphen && attrs.has(ClusterAttrs::BREAK_AFTER) {
            self.scratch.push_hyphen_node(pending.item.node, id);
        }
        if hyphen {
            self.out.push_hyphen(id);
            self.scratch.push_hyphen_node(pending.item.node, id);
            self.paragraph.flags.insert(ParagraphFlags::HAS_HYPHENS);
        }
        let started = self.script_runs.cluster(
            &mut self.scratch.pending_runs,
            id,
            pending.props,
            pending.item.language,
            pending.setting,
            id == self.paragraph.start,
        );
        self.paragraph.flags.insert(class.paragraph_flags());
        // A run has one orientation and lies in one paragraph, so its first
        // cluster sets the paragraph's orientation flags.
        if let Some(orientation) = started {
            self.paragraph
                .flags
                .insert(ParagraphFlags::from(orientation));
        }
        if class.is_forced_break() {
            self.end_paragraph();
        }
    }

    /// Returns what the host's segmenters make of the boundary after
    /// `pending`, of `class`, before `next`, both in the text item being
    /// walked: whether a line may break there, whether only where nothing
    /// else fits, and whether a break there draws a hyphen.
    ///
    /// `breaks` is the answer without them. A boundary the item shares with
    /// the text before keeps it. Out of line: few texts ask.
    #[inline(never)]
    fn hosted_break(
        &mut self,
        pending: &Pending,
        class: ClusterClass,
        next: &Pending,
        breaks: bool,
    ) -> (bool, bool, bool) {
        if pending.start < self.item_start {
            return (breaks, false, false);
        }
        let at = next.start;
        let points = &mut self.scratch.hosted;
        match self.hosted {
            Hosted::Hyphens => match points.take(at) {
                // The text's last word breaks only where nothing else fits.
                Some(true) if !breaks => (false, true, true),
                Some(false) if !breaks => (true, false, true),
                _ => (breaks, false, false),
            },
            // A soft hyphen breaks, with its hyphen, only where nothing else
            // fits.
            Hosted::Phrases | Hosted::Unsegmented
                if class == ClusterClass::SoftHyphen && breaks =>
            {
                (false, true, true)
            }
            Hosted::Phrases
                if breaks
                    && hosted::is_phrase_letter(pending.last)
                    && hosted::is_phrase_letter(next.first)
                    && points.take(at).is_none() =>
            {
                (false, true, false)
            }
            _ => (breaks, false, false),
        }
    }

    /// Returns the rule for the boundary between `pending` and `next`,
    /// neither of them a separator.
    ///
    /// Each part of a ruby column breaks under its own style, and part edges
    /// inside a column are not opportunities. A nested container's ended
    /// base decides the next boundary in its parent's base.
    fn boundary(&self, pending: &Pending, next: &Pending) -> Boundary {
        let unit = pending.setting.unit;
        let combined = unit != 0 && unit == next.setting.unit;
        let same_column = pending.column != 0 && pending.column == next.column;
        if same_column
            && !next.item.annotation
            && let Some(base) = self.ruby.inner_boundary
        {
            return Boundary::AfterNestedBase(base);
        }
        if same_column && pending.part == next.part && !combined {
            Boundary::InsidePart
        } else if same_column || combined {
            Boundary::Held
        } else if pending.column != 0 {
            Boundary::AfterColumn
        } else {
            Boundary::Text
        }
    }

    /// Returns whether a line may break after the ruby base `base`, before
    /// `next`.
    ///
    /// As Chrome's `CanBreakAfterRubyColumn` does, it segments the base text
    /// (without annotations) and the text after it, under the options of the
    /// text after. Then the filters apply, with the base's last cluster
    /// before the boundary. No line breaks after a column with no base, as in
    /// Chrome. Out of line ([`open_paragraph`](Self::open_paragraph)).
    #[inline(never)]
    fn breaks_after_base(&mut self, next: &Pending, base: RubyBase) -> bool {
        let (Some((start, first)), Some(last)) = (base.start, base.last) else {
            return false;
        };
        if next.continuation {
            return false;
        }
        let start = start.get();
        let key = next.item.key;
        let text = if key.reads_cj_quotes() {
            self.cj_text
        } else {
            self.text
        };
        // Gathers the base text, without any nested annotation, in scratch.
        let scratch = &mut self.scratch.column_text;
        scratch.clear();
        let base_end = last.end.get();
        let items_end = self.content.nodes.items(last.item.node).end;
        for item in self.content.items.slice(first..items_end) {
            if item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            let from = item.start.get().max(start);
            let to = item.end.get().min(base_end);
            if from < to {
                scratch.push_str(text.get(from..to).unwrap_or_default());
            }
        }
        let base = scratch.as_str();
        let following = text.get(next.start..).unwrap_or_default();
        // Reads a few characters of the following text, stopping at a
        // paragraph's end.
        let mut reach = 0;
        for (count, (offset, ch)) in following.char_indices().enumerate() {
            work::step();
            if count == COLUMN_LOOKAHEAD || unicode::core_props(ch).is_paragraph_separator() {
                break;
            }
            reach = offset + ch.len_utf8();
        }
        // Chrome's ASCII rules go first, as Blink's `LazyLineBreakIterator`
        // asks them before ICU.
        let mut base_chars = base.chars().rev();
        let (last_char, before) = (base_chars.next(), base_chars.next());
        let segmenter = self.cx.line(key);
        let answer = match (last_char, following.chars().next()) {
            (Some(last), Some(first)) => key.ascii_or(
                || before,
                last,
                first,
                || {
                    let length = scratch.len();
                    scratch.push_str(following.get(..reach).unwrap_or_default());
                    super::breaks::is_break_opportunity(segmenter, scratch, length)
                        || super::breaks::is_quotation_break(
                            segmenter,
                            scratch.get(..length).unwrap_or_default(),
                            following,
                        )
                },
            ),
            // With no base text or nothing after it, there is no pair.
            _ => false,
        };
        self.filter_break(&last.item, last.last, last.class, next, answer)
    }

    /// Returns whether a line may break between `pending`, of `class`, and
    /// `next`.
    ///
    /// Chrome's ASCII rules answer where they decide; otherwise the
    /// paragraph's line stream answers, or the ruby part's if `inner`. Then
    /// the filters apply.
    fn stream_breaks(
        &mut self,
        inner: bool,
        pending: &Pending,
        class: ClusterClass,
        next: &Pending,
    ) -> bool {
        // Never inside a grapheme, even one split by an item boundary.
        if next.continuation {
            return false;
        }
        let (from, stream) = if inner {
            (self.inner_start, &mut self.inner_stream)
        } else {
            (0, &mut self.stream)
        };
        // The character before the cluster's last, which the hyphen rule
        // reads; found only when asked.
        let before = self.text.get(from..next.start).unwrap_or_default();
        let cx = self.cx;
        let text = self.text;
        let key = next.item.key;
        let breaks = key.ascii_or(
            || before.chars().rev().nth(1),
            pending.last,
            next.first,
            || {
                stream.as_mut().is_some_and(|stream| stream.at(next.start))
                    || (super::breaks::may_break_beside_quotation(pending.first, next.first)
                        && super::breaks::is_quotation_break(
                            cx.line(key),
                            before,
                            text.get(next.start..).unwrap_or_default(),
                        ))
            },
        );
        self.filter_break(&pending.item, pending.last, class, next, breaks)
    }

    /// Starts the line stream of the ruby part (base or annotation) that
    /// begins at `at`.
    ///
    /// The stream's text ends where the part does, so dictionary context
    /// never reaches the next part. Style seams resume it like the
    /// paragraph's stream.
    #[inline(never)]
    fn begin_inner_part(&mut self, at: usize) {
        self.inner_start = at;
        let end = self
            .content
            .items
            .as_slice()
            .get(self.item_id.get()..)
            .unwrap_or_default()
            .iter()
            .find(|item| {
                matches!(
                    item.kind,
                    ItemKind::AnnotationOpen | ItemKind::AnnotationClose | ItemKind::RubyClose
                )
            })
            .map_or(self.text.len(), |item| item.start.get());
        self.inner_stream = Some(LineStream::new(
            self.text.get(..end).unwrap_or(self.text),
            self.cj_text.get(..end).unwrap_or(self.cj_text),
            at,
            self.cx.line(self.item.key),
            self.item.key,
        ));
    }

    /// Applies the style filters to `breaks`, the segmenter's answer between
    /// a cluster of `class` in `item` ending with `last`, and `next`.
    fn filter_break(
        &self,
        item: &ItemInfo,
        last: char,
        class: ClusterClass,
        next: &Pending,
        mut breaks: bool,
    ) -> bool {
        // If configured, a small kana or the prolonged sound mark is held to
        // what precedes it under `line-break: normal` and `auto`, as CSS Text 3 says
        // (csswg-drafts#10363). Line_Break CJ is read as NS, as under
        // `strict`, so it starts a line only after a space or a zero width
        // space (LB18, LB8); a tab and an ideographic space are BA. ICU, and
        // Chrome, read CJ as an ideograph under `normal`. This runs before
        // `break-spaces`, which breaks after every kept space regardless.
        if self.hold_small_kana
            && breaks
            && next.item.key.is_normal()
            && !matches!(
                class,
                ClusterClass::Space | ClusterClass::ZeroWidthSpace | ClusterClass::BreakOpportunity
            )
            && unicode::rare_props(next.first).is_conditional_japanese_starter()
        {
            breaks = false;
        }
        // `break-spaces` breaks after every preserved space and tab, even
        // between two, and after every other breaking space separator. Chrome
        // breaks between two ideographic spaces there, where UAX #14 does not
        // (WPT `break-spaces-with-ideographic-space-001`).
        if item.collapse == WhiteSpaceCollapse::BreakSpaces && class.is_breaking_space() {
            breaks = true;
        }
        // `hyphens: none`: a soft hyphen is no opportunity.
        if class == ClusterClass::SoftHyphen && item.hyphens_none {
            breaks = false;
        }
        // An atomic inline has an opportunity on each side, even where the
        // neighbouring character would suppress one, except beside GL other
        // than U+00A0, WJ or ZWJ (CSS Text 3, section 5.1). Chrome's answer
        // there is unmeasured.
        if self.any_atomics {
            if next.class == ClusterClass::Object {
                breaks = !glues(last);
            }
            if class == ClusterClass::Object {
                breaks = !glues(next.first);
            }
        }
        // An opportunity after a space or tab, which disappears or hangs at
        // the break, follows whether its own box wraps. One between other
        // characters follows their nearest common ancestor (CSS Text 3,
        // section 5.1). Chrome treats spaces and tabs so, and other space
        // separators like any character. A generated opportunity (a `<wbr>`,
        // or one kept where wrapping white space joins a `nowrap` run)
        // follows its own box, as Chrome's line breaker reads its item's
        // style.
        let wraps = if class.is_space_or_tab() || class == ClusterClass::BreakOpportunity {
            item.wraps
        } else {
            self.wraps_between(item, &next.item)
        };
        breaks && wraps
    }

    /// Returns whether text wraps between `before`, not a space or tab, and
    /// `after`.
    ///
    /// The `text-wrap-mode` of their nodes' nearest common ancestor decides
    /// (CSS Text 3, section 5.1).
    fn wraps_between(&self, before: &ItemInfo, after: &ItemInfo) -> bool {
        if !self.any_nowrap {
            return true;
        }
        if before.node == after.node {
            return before.wraps;
        }
        let nodes = &self.content.nodes;
        let mut node = before.node;
        // Bounded to one step per node, even if the parents form a cycle.
        for _ in 0..nodes.len() {
            if nodes.contains(node, after.node) {
                break;
            }
            let parent = nodes.parent(node);
            if parent == node {
                break;
            }
            node = parent;
        }
        let text = nodes.text_facts(node, FirstLineVariant::Standard);
        self.content.facts.text(text).has(TextFlags::WRAPS)
    }

    /// Ends the paragraph after the last cluster written. Out of line
    /// ([`open_paragraph`](Self::open_paragraph)).
    #[inline(never)]
    fn end_paragraph(&mut self) {
        let paragraph = &self.paragraph;
        let mut flags = paragraph.flags;
        if paragraph.east_asian {
            flags.insert(ParagraphFlags::HAS_EAST_ASIAN);
        }
        // Resolves the base level and any level changes. `bidi::resolve`
        // skips the full resolution unless something right to left or a
        // control can raise a level, or the block is right to left.
        let end = self.out.clusters.end_id();
        let (level, level_flags) = bidi::resolve(
            self.content,
            self.out,
            &mut self.scratch.bidi,
            paragraph,
            self.requested,
        );
        flags.insert(level_flags);
        let pushed = self.out.paragraphs.push(
            Paragraph {
                start: paragraph.start,
                level,
                flags,
            },
            end,
        );
        debug_assert!(pushed.is_some(), "no more paragraphs than clusters");
        self.out.flags.insert(flags);
        self.paragraph.open = false;
        self.stream = None;
    }

    /// Finishes the writer at the text's end.
    pub(super) fn finish(mut self) {
        if let Some(pending) = self.pending.take() {
            self.finish_cluster(pending, None, self.text.len());
        }
        // Every cluster is written, so the last item ends here.
        self.out.item_clusters.end = self.out.clusters.end_id();
        // Ends the open paragraph. If none is open (the text is empty or ends
        // in a separator), ends an empty one, where the caret sits after a
        // final `<br>`.
        if !self.paragraph.open {
            self.open_paragraph();
        }
        self.paragraph.controls |= self.controls;
        let empty = self.paragraph.start == self.out.clusters.end_id();
        self.end_paragraph();
        if empty {
            let end_line = end_opens_edged_box(self.content, self.out);
            self.out.paragraphs.set_end_line(end_line);
        }
        self.script_runs.finish(&mut self.scratch.pending_runs);
        scripts::emit(
            &self.scratch.pending_runs,
            &self.out.paragraphs,
            &self.scratch.bidi.changes,
            self.out.clusters.end_id(),
            &mut self.out.runs,
        );
        (self.out).set_control_levels(&self.scratch.bidi.control_levels);
    }
}

/// Returns whether the empty last paragraph opens a box with an inline
/// margin, border or padding.
///
/// Such a box makes a line where nothing else would, as Chrome's line
/// breaker makes one for an open tag with an inline edge. A box closing
/// there closes on the line before, after the forced break. Cold: most
/// texts end in a cluster.
#[cold]
#[inline(never)]
fn end_opens_edged_box(content: &Content, analysis: &Analysis) -> bool {
    let end = analysis.clusters.end_id();
    let edged = |item: &Item| {
        let facts = content
            .nodes
            .box_facts(item.node, FirstLineVariant::Standard);
        let facts = content.facts.box_facts(facts);
        let zero = (LayoutUnit::ZERO, LayoutUnit::ZERO);
        facts.margin_line != zero || facts.room != facts.margin_line
    };
    let mut id = ItemId::new(content.items.len());
    while let Some(before) = id.get().checked_sub(1) {
        work::step();
        id = ItemId::new(before);
        let range = analysis.item_clusters.range(id);
        if range.start != end || !range.is_empty() {
            break;
        }
        let Some(item) = content.items.get(id) else {
            break;
        };
        let opens = matches!(item.kind, ItemKind::Open | ItemKind::RubyOpen);
        if opens && !item.flags.contains(ItemFlags::ANNOTATION) && edged(item) {
            return true;
        }
    }
    false
}
