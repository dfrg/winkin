//! The cluster writer: the one pass over the items and the text that writes
//! the analysis.
//!
//! Start at: [`ClusterWriter::visit_item`], once per item in reading order,
//! then [`ClusterWriter::finish`]. A text-bearing item's characters go
//! through `ClusterWriter::text_item`, and each cluster through
//! `ClusterWriter::start_cluster` and `ClusterWriter::finish_cluster`.
//!
//! `classify` holds the per-character classifiers: a cluster's class, the
//! Latin-1 fast path, hanging, glue and vertical orientation.
//!
//! - **Each cluster is written once, whole,** when the next one starts. Its
//!   end is known then: the line break answer at the boundary, the item and
//!   character after it, and any zero-length items between (box edges,
//!   floats, ruby marks) that stop shaping there.
//! - **A cluster starts** at every grapheme boundary, every text item's start
//!   and after every U+FFFC.
//! - **A paragraph ends** after a separator. Its line stream starts at its
//!   first cluster and resumes where a text item changes the break options.
//!   Its base level and flags are written when it ends.

use core::mem;

use icu_segmenter::iterators::GraphemeClusterBreakIterator;
use icu_segmenter::scaffold::Utf8;

use super::breaks::{LineKey, LineStream};
use super::classify::is_east_asian;
use super::classify::{
    LEVEL_CLASSES, first_class, glues, hangs, is_latin1_fast, latin1_fast_char, latin1_fast_class,
};
use super::scripts::{self, Runs, Setting};
use super::{
    Analysis, AnalysisContext, AnalysisInput, AnalysisScratch, BidiLevel, ClusterAttrs,
    ClusterClass, ClusterId, Paragraph, ParagraphFlags, RunOrientation, levels,
};
use crate::config::SmallKana;
use crate::data::{Id, TextOffset, define_flags};
use crate::stages::content::{
    Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind, LanguageId, MAX_RUBY_DEPTH, NodeId,
    TextFlags, TextSetting,
};
use crate::style::{Direction, FirstLineVariant, TextCombineUpright, WhiteSpaceCollapse};
use crate::unicode::{self, CoreProps, RareProps};
use crate::work;

/// U+FFFC OBJECT REPLACEMENT CHARACTER. A cluster always ends after one, so a
/// combining mark after an atomic inline does not join it.
const OBJECT: char = '\u{FFFC}';

/// What the writer knows of the text-bearing item a cluster is in: its node
/// and what its style says about breaking.
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
    /// `overflow-wrap: anywhere` or `break-word`, and the text wraps.
    emergency: bool,
    /// Inside a ruby annotation.
    annotation: bool,
    /// A break opportunity the builder generated, whose U+200B is a class
    /// of its own.
    pub(super) generated: bool,
    /// How its text stands in the block's lines.
    setting: TextSetting,
    /// What of its text `text-combine-upright` combines, in a vertical
    /// line: nothing elsewhere.
    combine: TextCombineUpright,
}

impl ItemInfo {
    /// What the writer knows of `item`, a text-bearing item of `content`.
    fn new(content: &Content, item: &Item) -> Self {
        Self {
            kind: item.kind,
            annotation: item.flags.contains(ItemFlags::ANNOTATION),
            generated: item.flags.contains(ItemFlags::GENERATED),
            ..Self::from_node(content, item.node)
        }
    }

    /// What the writer takes the block itself to be before any item: its own
    /// style, for a content with no items at all.
    pub(super) fn from_block(content: &Content) -> Self {
        Self {
            emergency: false,
            ..Self::from_node(content, NodeId::BLOCK)
        }
    }

    /// Text with `node`'s text facts: what [`new`](Self::new) and
    /// [`from_block`](Self::from_block) share.
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

    /// Whether the item sets the options its text breaks under. Only text
    /// does: an atomic inline's U+FFFC and a `<br>`'s `\n` break under the
    /// options around them, so a stream does not resume at each.
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
    /// What makes a cluster an emoji, whatever its first character: a
    /// keycap, a modifier, a ZWJ sequence, a tag or a flag.
    const EMOJI: Self =
        Self(Self::KEYCAP.0 | Self::MODIFIER.0 | Self::SEQUENCE.0 | Self::TAG.0 | Self::FLAG.0);

    /// Whether any flag in `other` is set, where `contains` asks for all.
    fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// The cluster being gathered: finished when the next one starts.
#[derive(Copy, Clone, Debug)]
struct Pending {
    start: usize,
    item: ItemInfo,
    /// Its class from its first character.
    class: ClusterClass,
    /// Its first character's core word: its script and bidi class.
    props: CoreProps,
    first: char,
    last: char,
    marks: EmojiMarks,
    /// It holds a U+200C ZERO WIDTH NON-JOINER, which Blink ends a shaping
    /// run at where its white space is preserved.
    zwnj: bool,
    /// A character after its first is a variation selector, VS1 to VS256.
    selector: bool,
    /// A zero-length item after it stops shaping at its end.
    shape_break: bool,
    /// It is the later part of a grapheme an item boundary divided.
    continuation: bool,
    /// The ruby column it is in, its base or an annotation: a number the
    /// writer gives each column in turn, and 0 outside every column.
    column: u32,
    /// The annotation part within its column, or zero for its base. Distinct
    /// annotations never share an inner wrap opportunity.
    part: u32,
    /// How it stands in its line, and the combined unit it is in: a number
    /// the writer gives each unit in turn, and 0 outside every unit.
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

    /// A character after the first joins the cluster.
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

    /// Returns its class, with the characters after its first taken into
    /// account.
    ///
    /// A keycap makes a digit an emoji. A text variation selector makes an
    /// emoji a symbol. A variation selector 16, a modifier, a tag or a ZWJ
    /// sequence makes a symbol an emoji.
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

/// The paragraph being gathered.
#[derive(Default)]
struct ParagraphState {
    /// Whether one has begun and not ended.
    open: bool,
    start: ClusterId,
    /// The item holding its first cluster, where its bidi input starts.
    first_item: ItemId,
    flags: ParagraphFlags,
    /// A style synthesizes a bidi control in it: a box with `unicode-bidi`
    /// or an annotation opens or closes in it, or is open across its start,
    /// or the block overrides bidi.
    controls: bool,
    /// Every bidi class of its characters, a bit each.
    classes: u32,
    east_asian: bool,
}

/// Only the facts a closing-base filter reads; styles are recovered once
/// at close, rather than retaining a full Pending per enclosing base.
#[derive(Copy, Clone, Debug)]
struct BaseTail {
    node: NodeId,
    last: char,
    class: ClusterClass,
    end: TextOffset,
}
impl BaseTail {
    fn new(pending: Pending, end: usize) -> Self {
        Self {
            node: pending.item.node,
            last: pending.last,
            class: pending.class(),
            end: TextOffset::new(end),
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct RubyBase {
    start: Option<(TextOffset, ItemId)>,
    last: Option<BaseTail>,
    after_annotation: bool,
}

/// One stack for root and nested bases. A child hands its final base tail
/// to its parent when it closes; clusters update only the current base.
#[derive(Copy, Clone, Debug, Default)]
struct RubyState {
    containers: u32,
    bases: [RubyBase; MAX_RUBY_DEPTH],
    inner_boundary: Option<RubyBase>,
    annotations: u32,
    part: u32,
    serial: u32,
    column: u32,
    seen: u32,
}

/// Where the writer is among the units `text-combine-upright` combines.
///
/// Each unit is set as one character and one run, and no line breaks inside
/// it.
#[derive(Copy, Clone, Debug, Default)]
struct CombineState {
    /// The number the last unit was given.
    serial: u16,
    /// Under `all`, the unit the next cluster of the text item being walked
    /// joins: the text of one item, and of the text items straight after it,
    /// as Blink combines a text node and its text siblings in one
    /// `LayoutTextCombine`. 0 where the next cluster starts one.
    all: u16,
    /// Under `digits`, the unit the digits being walked are, and how many
    /// clusters of it are still to come.
    digits: Option<(u16, usize)>,
}

impl CombineState {
    /// Returns a number for the next unit: the last one plus one.
    ///
    /// It wraps past 0, which names no unit, so it never repeats the unit
    /// before it (`Setting::unit`).
    fn next_unit(&mut self) -> u16 {
        self.serial = self.serial.checked_add(1).unwrap_or(1);
        self.serial
    }
}

/// How many characters past a ruby column decide a break after it.
///
/// UAX #14 looks at a few characters after a boundary at most (LB25's
/// numbers, LB28a's aksaras), so a few more are enough. A paragraph of
/// columns then costs the same per column however long it is.
const COLUMN_LOOKAHEAD: usize = 8;

/// The writer's state, over one content.
pub(super) struct ClusterWriter<'a> {
    content: &'a Content,
    text: &'a str,
    cx: &'a AnalysisContext,
    scratch: &'a mut AnalysisScratch,
    out: &'a mut Analysis,
    /// The grapheme segmenter, over the text from `graphemes_from`.
    graphemes: GraphemeClusterBreakIterator<'static, 'a, Utf8>,
    graphemes_from: usize,
    /// The next grapheme boundary the segmenter found, at or after the last
    /// position it was asked about.
    next_grapheme: usize,
    /// The last grapheme boundary known, from the segmenter or between two
    /// characters that stand alone: where the segmenter starts again.
    known_grapheme: usize,
    /// The character before the one being visited, or [`char::MAX`] at the
    /// text's start.
    before: char,
    /// Whether that character's Grapheme_Cluster_Break stands alone: false
    /// at the text's start, where the segmenter is asked.
    before_alone: bool,
    stream: Option<LineStream<'a>>,
    /// The current ruby part, bounded before the next part's text.
    inner_stream: Option<LineStream<'a>>,
    inner_start: usize,
    pending: Option<Pending>,
    /// The text-bearing item the characters are from.
    item: ItemInfo,
    /// The item being walked, which holds a cluster starting now.
    item_id: ItemId,
    /// Where the text item being walked ends in the text.
    item_end: usize,
    /// The last character was U+FFFC.
    after_object: bool,
    paragraph: ParagraphState,
    runs: Runs,
    /// A box with `unicode-bidi`, or a ruby annotation, opened or closed
    /// since the last cluster: the paragraph of the next has controls. Those
    /// open are the bidi input's, which a paragraph starting inside one
    /// re-opens after a forced break.
    controls: bool,
    /// The block overrides bidi, which every paragraph starts with.
    block_overrides: bool,
    /// The level the block's direction sets its paragraphs at
    /// (`BlockFacts::direction`). It is `None` under `auto`, where each
    /// paragraph's first strong character decides.
    requested: Option<BidiLevel>,
    /// Some style is `nowrap`, so a break between two nodes asks whose style
    /// decides it.
    any_nowrap: bool,
    /// The content has atomic inlines, whose edges have an opportunity rule
    /// of their own.
    any_atomics: bool,
    /// A small kana is held to what precedes it under `line-break: normal`,
    /// as CSS Text now says, rather than starting a line, as in Chrome
    /// (`Config::small_kana`).
    hold_small_kana: bool,
    /// Where the writer is among the ruby columns.
    ruby: RubyState,
    /// Where the writer is among the combined units.
    combine: CombineState,
}

impl<'a> ClusterWriter<'a> {
    /// Starts the writer over the content of `input`, writing into `out`.
    pub(super) fn new(
        input: &AnalysisInput<'a>,
        cx: &'a AnalysisContext,
        scratch: &'a mut AnalysisScratch,
        out: &'a mut Analysis,
    ) -> Self {
        let AnalysisInput {
            content,
            small_kana,
        } = *input;
        let text = content.text.as_str();
        let flags = content.flags;
        let mut graphemes = cx.graphemes().segment_str(text);
        // The segmenter says 0 first: the text's start is a boundary.
        let next_grapheme = graphemes.next().unwrap_or(usize::MAX);
        // Until the first text-bearing item, which comes before any cluster.
        let item = ItemInfo::from_block(content);
        Self {
            content,
            text,
            cx,
            scratch,
            out,
            graphemes,
            graphemes_from: 0,
            next_grapheme,
            known_grapheme: 0,
            before: char::MAX,
            before_alone: false,
            stream: None,
            inner_stream: None,
            inner_start: 0,
            pending: None,
            item,
            item_id: ItemId::default(),
            item_end: 0,
            after_object: false,
            paragraph: ParagraphState::default(),
            runs: Runs::new(),
            controls: false,
            // A left-to-right override raises even levels over even ones,
            // which Blink leaves at 0 (`levels::moves_levels`).
            block_overrides: content.block.overrides == Some(Direction::Rtl),
            requested: BidiLevel::from_direction(content.block.direction),
            any_nowrap: flags.contains(ContentFlags::NOWRAP),
            any_atomics: flags.contains(ContentFlags::ATOMICS),
            hold_small_kana: small_kana == SmallKana::Held,
            ruby: RubyState::default(),
            combine: CombineState::default(),
        }
    }
}

impl ClusterWriter<'_> {
    /// Returns the id the next cluster takes; the pending one is not yet
    /// written.
    fn next_cluster(&self) -> ClusterId {
        ClusterId::new(self.out.clusters.len() + usize::from(self.pending.is_some()))
    }

    /// Walks item `id`: records its first cluster, then walks its characters
    /// or applies what it does to the clusters beside it.
    pub(super) fn visit_item(&mut self, id: ItemId, item: &Item) {
        self.item_id = id;
        let first = self.next_cluster();
        if self.out.item_clusters.push(first).is_none() {
            debug_assert!(false, "one entry per item, which an ItemId names");
        }
        // Only a text item's text goes on with a combined unit: an edge, an
        // atomic inline or anything else between ends it.
        if item.kind != ItemKind::Text {
            self.combine.all = 0;
            self.combine.digits = None;
        }
        match item.kind {
            ItemKind::Text | ItemKind::Atomic | ItemKind::Break => self.text_item(item),
            ItemKind::Open | ItemKind::Close => self.edge(item),
            // An annotation's content is an isolate of its own, so it adds
            // controls, as a box's `unicode-bidi` does.
            ItemKind::AnnotationOpen | ItemKind::AnnotationClose => {
                self.controls |= self.scratch.bidi.walk(self.content, item);
                let ruby = &mut self.ruby;
                if item.kind == ItemKind::AnnotationOpen {
                    if ruby.containers > 0 {
                        let base = &mut ruby.bases[ruby.containers as usize - 1];
                        if let Some(pending) = self.pending.filter(|pending| {
                            !pending.item.annotation
                                && base
                                    .start
                                    .is_some_and(|(start, _)| pending.start >= start.get())
                        }) {
                            base.last = Some(BaseTail::new(pending, item.start.get()));
                        }
                    }
                    ruby.annotations = ruby.annotations.saturating_add(1);
                    ruby.part = ruby.part.saturating_add(1);
                } else {
                    ruby.annotations = ruby.annotations.saturating_sub(1);
                    if ruby.annotations == 0 && ruby.containers > 0 {
                        let base = &mut ruby.bases[ruby.containers as usize - 1];
                        if ruby.containers > 1 {
                            ruby.inner_boundary = Some(*base);
                        }
                        base.after_annotation = true;
                    }
                }
                self.stop_shaping();
            }
            ItemKind::RubyOpen => {
                // The container's `unicode-bidi` adds controls, as a box's
                // does.
                self.controls |= self.scratch.bidi.walk(self.content, item);
                self.stop_shaping();
                // A root container with no annotation is no column: its text
                // is the line's, as a box's is. Its close finds no container
                // open.
                if self.ruby.containers == 0
                    && !self.content.annotation_follows(ItemId::new(id.get() + 1))
                {
                    return;
                }
                // A column starts with the container.
                let ruby = &mut self.ruby;
                ruby.containers = ruby.containers.saturating_add(1);
                // A root's first cluster retires the previous column's
                // tail after deciding its boundary. Opening a sibling
                // must leave that tail available until then.
                if ruby.containers > 1 {
                    ruby.bases[ruby.containers as usize - 1] = RubyBase {
                        start: Some((item.start, id)),
                        ..RubyBase::default()
                    };
                }
                if ruby.containers == 1 {
                    ruby.serial = ruby.serial.saturating_add(1);
                    ruby.column = ruby.serial;
                    ruby.bases[0].after_annotation = false;
                }
            }
            ItemKind::RubyClose => {
                self.controls |= self.scratch.bidi.walk(self.content, item);
                let ruby = &mut self.ruby;
                if ruby.containers > 0 {
                    let depth = ruby.containers as usize - 1;
                    let mut base = ruby.bases[depth];
                    if let Some(pending) = self.pending.filter(|pending| !pending.item.annotation)
                        && base
                            .start
                            .is_some_and(|(start, _)| pending.start >= start.get())
                    {
                        base.last = Some(BaseTail::new(pending, item.start.get()));
                    }
                    ruby.bases[depth] = base;
                    if depth > 0 {
                        ruby.inner_boundary = Some(base);
                        if base.last.is_some() {
                            ruby.bases[depth - 1].last = base.last;
                        }
                    }
                }
                ruby.containers = ruby.containers.saturating_sub(1);
                if ruby.containers == 0 {
                    ruby.column = 0;
                    ruby.annotations = 0;
                    ruby.bases[0].after_annotation = false;
                }
                self.stop_shaping();
            }
            ItemKind::Float | ItemKind::Absolute => self.stop_shaping(),
        }
    }

    /// Returns the ruby column of a cluster starting now, and whether it is
    /// the column's first.
    ///
    /// Base text after an annotation starts the container's next column,
    /// where an annotation follows it. Without one it is the line's text.
    fn next_column(&mut self) -> (u32, bool) {
        let ruby = &mut self.ruby;
        if ruby.containers == 0 {
            return (0, false);
        }
        if !self.item.annotation && ruby.bases[0].after_annotation {
            ruby.bases[0].after_annotation = false;
            if self.content.annotation_follows(self.item_id) {
                ruby.serial = ruby.serial.saturating_add(1);
                ruby.column = ruby.serial;
            } else {
                ruby.column = 0;
            }
        }
        (ruby.column, ruby.column != ruby.seen)
    }

    /// Handles a box's opening or closing edge, which stops shaping where its
    /// box facts say so (`BoxFacts::breaks_shaping`).
    fn edge(&mut self, item: &Item) {
        self.controls |= self.scratch.bidi.walk(self.content, item);
        let content = self.content;
        let id = content
            .nodes
            .box_facts(item.node, FirstLineVariant::Standard);
        let opens = item.kind == ItemKind::Open;
        if content.facts.box_facts(id).breaks_shaping(opens) {
            self.stop_shaping();
        }
    }

    /// EmojiMarks a shaping stop at the boundary here, on the cluster ending here.
    ///
    /// A separator stops shaping anyway, and before the text's start there is
    /// no cluster to mark.
    fn stop_shaping(&mut self) {
        if let Some(pending) = &mut self.pending
            && !pending.class.is_forced_break()
        {
            pending.shape_break = true;
            self.paragraph
                .flags
                .insert(ParagraphFlags::HAS_SHAPING_STOPS);
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
        self.item_end = item.end.get();
        let Some(text) = self.text.get(start..item.end.get()) else {
            debug_assert!(false, "items fall on character boundaries");
            return;
        };
        // An atomic inline is a cluster of its own on both sides: never the
        // rest of what comes before it, as a prepended mark would make it.
        let object = self.item.kind == ItemKind::Atomic;
        // The Latin-1 fast path is open in text set across a horizontal line
        // outside ruby.
        let fast_path = work::fast_paths()
            && self.item.kind == ItemKind::Text
            && self.item.setting == TextSetting::Horizontal
            && self.ruby.containers == 0;
        let mut chars = text.char_indices();
        // Where `chars` starts in the item's text: the fast path moves it on.
        let mut base = 0;
        while let Some((offset, ch)) = chars.next() {
            let offset = base + offset;
            let at = start + offset;
            let props = unicode::core_props(ch);
            // The rare word, read once a character past Latin-1, where the
            // grapheme step always asks it and a cluster's class and
            // orientation may; Latin-1 reads none for either.
            let rare = (ch >= '\u{100}').then(|| unicode::rare_props(ch));
            let grapheme = self.grapheme_boundary(at, ch, rare);
            let starts = offset == 0 || grapheme || self.after_object;
            // A cluster that starts only because an item does is the rest of
            // a grapheme the item boundary divided.
            let continuation = !grapheme && !self.after_object && !object;
            if starts {
                self.start_cluster(at, ch, props, rare, continuation);
            } else if let Some(pending) = &mut self.pending {
                pending.extend(ch, props);
            }
            let paragraph = &mut self.paragraph;
            paragraph.classes |= props.bidi_class().mask();
            paragraph.east_asian |= is_east_asian(ch);
            self.after_object = ch == OBJECT;
            // A fast-path character that started a cluster of its own: the
            // characters after it go by the fast path while they are
            // fast-path characters, and the walk goes on where it stops.
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

    /// Walks fast-path characters fast, from byte `from` of `text`, while each is
    /// one ([`latin1_fast_char`]). Returns where it stops, in `text`.
    ///
    /// `text` is the item being walked and starts at byte `start` of the
    /// whole text. `first` is the character at `from`. The pending cluster is
    /// a fast-path character that started a cluster of its own, in this item.
    ///
    /// Each cluster the fast path finishes is one fast-path character, of its own class
    /// ([`latin1_fast_class`]) with no marks. There is always a grapheme boundary
    /// between two fast-path characters. The item is horizontal text outside ruby,
    /// the paragraph is open, its line stream breaks under the item's key, and
    /// no control is waiting.
    ///
    /// So each cluster is written as [`finish_cluster`](Self::finish_cluster)
    /// writes it, minus the parts that come to nothing here:
    /// - the ruby column, the combined unit and the stream's resume;
    /// - a ZWNJ, and a stop from an item between;
    /// - the held small kana (no Latin-1 character is one);
    /// - a soft hyphen, an atomic's edges and a separator;
    /// - the paragraph flags and the East Asian gate.
    ///
    /// The runs are asked only where the cluster changes them
    /// ([`Runs::joins`]). The last character is left pending, as the general
    /// walk leaves it, so a mark after it joins it and the grapheme step goes
    /// on from it.
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
        let (mut last, mut last_class, mut last_hot) =
            (pending.first, pending.class, pending.props);
        let mut last_start;
        let mut offset = from;
        loop {
            let at = start + offset;
            let props = unicode::core_props(ch);
            // The character before the last, which a hyphen's rule reads.
            let before_last = || {
                whole
                    .get(..at)
                    .and_then(|before| before.chars().rev().nth(1))
            };
            let ascii = key.ascii_answer(before_last, last, ch);
            let mut breaks = match ascii {
                Some(breaks) => breaks,
                None => self.stream.as_mut().is_some_and(|stream| stream.at(at)),
            };
            let space = last_class == ClusterClass::Space;
            breaks |= break_spaces && space;
            // Whether text wraps at a boundary inside one item is the item's
            // own wrapping, after a space as after anything else: where
            // `filter_break` reads no style, no style is `nowrap`.
            breaks &= item.wraps;
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
            let opens = id == self.paragraph.start;
            if opens || !self.runs.joins(last_hot, item.language) {
                let started = self.runs.cluster(
                    &mut self.scratch.runs,
                    id,
                    last_hot,
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
            self.paragraph.classes |= props.bidi_class().mask();
            (last, last_class, last_hot, last_start) = (ch, latin1_fast_class(ch), props, at);
            offset += len;
            match latin1_fast_char(bytes, offset) {
                Some((next, next_len)) => (ch, len) = (next, next_len),
                None => break,
            }
        }
        // The last character is pending, as the general walk leaves it.
        self.pending = Some(Pending::new(last_start, item, last, last_hot, None, false));
        self.before = last;
        self.before_alone = true;
        self.known_grapheme = last_start;
        offset
    }

    /// Whether there is a grapheme boundary at `at`, before `ch`.
    ///
    /// `rare` is `ch`'s rare word past Latin-1. Every character is asked
    /// about, in order.
    ///
    /// Between two characters whose Grapheme_Cluster_Break stands alone
    /// (Other, a control, CR or LF) there is always a boundary, except inside
    /// a CRLF. Every rule of UAX #29 that keeps two characters together names
    /// another class on one side. Every Latin-1 character stands alone, so
    /// Latin-1 text reads no table for it. Ideographs, kana and most letters
    /// beyond Latin-1 read their rare word.
    ///
    /// So only a character that may join a neighbour, or the one after it,
    /// asks the segmenter. Where the segmenter has fallen behind, it restarts
    /// at the last known boundary. Like the line segmenter, it goes on from a
    /// boundary as from the text's start.
    fn grapheme_boundary(&mut self, at: usize, ch: char, rare: Option<RareProps>) -> bool {
        let before = mem::replace(&mut self.before, ch);
        let alone = rare.is_none_or(|rare| rare.grapheme_cluster_break().stands_alone());
        let before_alone = mem::replace(&mut self.before_alone, alone);
        if alone && before_alone && !(before == '\r' && ch == '\n') {
            self.known_grapheme = at;
            return true;
        }
        if self.next_grapheme < self.known_grapheme {
            let from = self.known_grapheme;
            self.graphemes = self
                .cx
                .graphemes()
                .segment_str(self.text.get(from..).unwrap_or_default());
            // It says 0 first, which is `from`.
            self.graphemes.next();
            self.graphemes_from = from;
            self.next_grapheme = from;
        }
        while self.next_grapheme < at {
            self.next_grapheme = match self.graphemes.next() {
                Some(next) => self.graphemes_from.saturating_add(next),
                None => usize::MAX,
            };
        }
        let boundary = self.next_grapheme == at;
        if boundary {
            self.known_grapheme = at;
        }
        boundary
    }

    /// Returns how a cluster stands in its line and its combined unit, and
    /// moves the writer among the units.
    ///
    /// The cluster starts at byte `at` with `first`, of class `class`, with
    /// the words `props` and `rare` as the writer read them. It is a
    /// `continuation` where an item boundary divided its grapheme.
    ///
    /// - **A continuation** stands as the part before it, since a grapheme
    ///   stands as one.
    /// - **Under `all`**, a forced break is no part of a unit, and the text
    ///   after it starts another.
    /// - **Under `digits`**, a unit is a maximal run of ASCII digits, no
    ///   longer than the style allows, all in the item. A longer run, or one
    ///   an item boundary divides, is set as digits are anywhere else.
    ///
    /// The writer asks this only of text in a vertical line. It takes values,
    /// not the pending cluster, which the cluster loop keeps out of memory.
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
    /// style combines runs of `most` digits or fewer.
    ///
    /// That is the open unit, or a new one where a run starts here, or 0.
    fn digits(&mut self, at: usize, most: usize) -> u16 {
        if let Some((unit, left)) = self.combine.digits
            && left > 0
        {
            self.combine.digits = Some((unit, left - 1));
            return unit;
        }
        self.combine.digits = None;
        // A digit after a digit is inside a run that did not combine.
        let before = self
            .text
            .get(..at)
            .and_then(|text| text.chars().next_back());
        if before.is_some_and(|ch| ch.is_ascii_digit()) {
            return 0;
        }
        // At most one more than the style allows, to tell a run too long.
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

    /// Starts a cluster at `at` with `ch`, and finishes the one before.
    ///
    /// `props` and, past Latin-1, `rare` are `ch`'s words.
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
        // Text across a horizontal line stands one way, and is in no unit:
        // asked here, so that the cluster loop of nearly every layout pays
        // one test for it.
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
            // resumes before it is asked about the boundary.
            if !pending.class.is_forced_break()
                && self.item.sets_key()
                && let Some(stream) = &mut self.stream
                && stream.key() != self.item.key
            {
                stream.resume(self.cx.line(self.item.key), self.item.key, at);
            }
            if pending.column != 0
                && pending.column == next.column
                && pending.part == next.part
                && self.item.sets_key()
                && let Some(stream) = &mut self.inner_stream
                && stream.key() != self.item.key
            {
                stream.resume(self.cx.line(self.item.key), self.item.key, at);
            }
            self.finish_cluster(pending, Some(&next), at);
        }
        // The column the cluster starts is its own from here: what a break
        // after the last was decided by is done with.
        if first_in_column {
            let ruby = &mut self.ruby;
            ruby.seen = column;
            ruby.bases[0] = RubyBase::default();
        }
        if column != 0 && !next.item.annotation && self.ruby.bases[0].start.is_none() {
            self.ruby.bases[0].start = Some((TextOffset::new(at), self.item_id));
        }
        if !next.item.annotation && self.ruby.containers > 1 {
            let base = &mut self.ruby.bases[self.ruby.containers as usize - 1];
            if base.after_annotation {
                *base = RubyBase {
                    start: Some((TextOffset::new(at), self.item_id)),
                    ..RubyBase::default()
                };
            }
        }
        self.ruby.inner_boundary = None;
        if !self.paragraph.open {
            self.open_paragraph();
            let key = self.item.key;
            self.stream = Some(LineStream::new(self.text, at, self.cx.line(key), key));
        }
        self.paragraph.controls |= mem::take(&mut self.controls);
        if starts_part {
            self.begin_inner_part(at);
        } else if column == 0 {
            self.inner_stream = None;
        }
        self.pending = Some(next);
    }

    /// Opens a paragraph at the next cluster, in the item being walked.
    ///
    /// It has controls where boxes open across its start re-open them, or
    /// where the block overrides bidi.
    ///
    /// Kept out of line, as a paragraph's end and a ruby column's break are.
    /// Each runs once per paragraph or column, and inlining them slows the
    /// cluster loop by 2 to 3%.
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

    /// Writes `pending`, which ends at `end`: where `next` starts, or at the
    /// text's end where there is no `next`.
    fn finish_cluster(&mut self, pending: Pending, next: Option<&Pending>, end: usize) {
        let class = pending.class();
        let mut attrs = ClusterAttrs::new(class);
        // A paragraph's last cluster offers no opportunity: its end is the
        // paragraph's. Nor does the one before a separator, which ends the
        // line there anyway, taking nothing wider with it; a filter that
        // adds opportunities (`break-spaces`, an atomic inline's) could
        // otherwise leave the separator a line of its own.
        if let Some(next) = next
            && !class.is_forced_break()
            && !next.class.is_forced_break()
        {
            let unit = pending.setting.unit;
            let combined = unit != 0 && unit == next.setting.unit;
            let same_column = pending.column != 0 && pending.column == next.column;
            // Resolve each part under its own style. Part edges are not
            // internal opportunities; the column's closing edge uses its base.
            let internal = same_column && pending.part == next.part && !combined;
            let inner_end = self
                .ruby
                .inner_boundary
                .filter(|_| same_column && !next.item.annotation);
            let breaks = if let Some(base) = inner_end {
                match (base.start, base.last) {
                    (Some((start, first)), Some(last)) => {
                        self.breaks_after_base(next, start.get(), first, last)
                    }
                    _ => false,
                }
            } else if internal {
                self.inner_breaks_between(&pending, class, next)
            } else if same_column || combined {
                false
            } else if pending.column != 0 {
                self.breaks_after_column(next)
            } else {
                self.breaks_between(&pending, class, next)
            };
            // An emergency break between two items opens where either allows
            // one, as Chrome's retry breaks before the first character of
            // text that allows `overflow-wrap`.
            attrs = attrs.with(ClusterAttrs::BREAK_AFTER, breaks).with(
                ClusterAttrs::EMERGENCY_AFTER,
                inner_end.map_or(pending.item.emergency || next.item.emergency, |base| {
                    base.last
                        .is_some_and(|last| ItemInfo::from_node(self.content, last.node).emergency)
                }) && !next.continuation
                    && (inner_end.is_some() || internal || (!same_column && !combined)),
            );
        }
        // A ZWNJ stops shaping only where its white space is preserved, as
        // Blink makes one a control item only under `preserve` and
        // `break-spaces`. Elsewhere the shaper kerns past it and ligates up
        // to it. The stop goes after it, at its cluster's end: a kern or a
        // ligature cannot tell before from after.
        let zwnj_stops = pending.zwnj && pending.item.collapse.keeps_spaces();
        if zwnj_stops {
            self.paragraph
                .flags
                .insert(ParagraphFlags::HAS_SHAPING_STOPS);
        }
        attrs = attrs
            // An annotation's white space is on its own line, never at the
            // end of the base's, and a combined unit's is inside its one
            // character.
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
        let started = self.runs.cluster(
            &mut self.scratch.runs,
            id,
            pending.props,
            pending.item.language,
            pending.setting,
            id == self.paragraph.start,
        );
        self.paragraph.flags.insert(class.paragraph_flags());
        // A run stands one way throughout, and starts in its paragraph: the
        // paragraph learns how from each run's first cluster.
        if let Some(orientation) = started {
            self.paragraph
                .flags
                .insert(ParagraphFlags::from(orientation));
        }
        if class.is_forced_break() {
            self.end_paragraph();
        }
    }

    /// Whether a line may break after a ruby column, before `next`: as
    /// Chrome's `CanBreakAfterRubyColumn` decides it, by the column's base
    /// and what follows the column, the annotations between them left out,
    /// under the options of what follows; then the filters,
    /// with the base's last cluster before the boundary. A column with no
    /// base has nothing to decide it by, and no line breaks after it, as in
    /// Chrome. Out of line ([`open_paragraph`](Self::open_paragraph)).
    #[inline(never)]
    fn breaks_after_column(&mut self, next: &Pending) -> bool {
        let base = self.ruby.bases[0];
        let (Some((start, first)), Some(last)) = (base.start, base.last) else {
            return false;
        };
        self.breaks_after_base(next, start.get(), first, last)
    }

    fn breaks_after_base(
        &mut self,
        next: &Pending,
        start: usize,
        first: ItemId,
        last: BaseTail,
    ) -> bool {
        if next.continuation {
            return false;
        }
        // The logical base excludes every child's annotation. Keep this
        // context in the existing scratch used for ruby-boundary segmentation.
        let scratch = &mut self.scratch.column_text;
        scratch.clear();
        let base_end = last.end.get();
        let items_end = self.content.nodes.items(last.node).end;
        for item in self.content.items.slice(first..items_end) {
            if item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            let from = item.start.get().max(start);
            let to = item.end.get().min(base_end);
            if from < to {
                scratch.push_str(self.text.get(from..to).unwrap_or_default());
            }
        }
        let base = scratch.as_str();
        let following = self.text.get(next.start..).unwrap_or_default();
        // A few characters of what follows, stopping at a paragraph's end.
        let mut reach = 0;
        for (count, (offset, ch)) in following.char_indices().enumerate() {
            work::step();
            if count == COLUMN_LOOKAHEAD || unicode::core_props(ch).is_paragraph_separator() {
                break;
            }
            reach = offset + ch.len_utf8();
        }
        // Chrome's ASCII rules first, over the base and what follows, as
        // Blink's `LazyLineBreakIterator` asks them before ICU.
        let mut base_chars = base.chars().rev();
        let key = next.item.key;
        let ascii = base_chars
            .next()
            .zip(following.chars().next())
            .and_then(|(last, first)| key.ascii_answer(|| base_chars.next(), last, first));
        let answer = match ascii {
            Some(answer) => answer,
            None => {
                let length = scratch.len();
                scratch.push_str(following.get(..reach).unwrap_or_default());
                super::breaks::is_break_opportunity(self.cx.line(next.item.key), scratch, length)
            }
        };
        let item = ItemInfo::from_node(self.content, last.node);
        self.filter_break(&item, last.last, last.class, next, answer)
    }

    /// Whether a line may break between `pending`, of `class`, and `next`.
    ///
    /// Chrome's own ASCII rules answer where they decide, and the line stream
    /// answers where they do not. Then the filters apply.
    fn breaks_between(&mut self, pending: &Pending, class: ClusterClass, next: &Pending) -> bool {
        // Never inside a grapheme, even one an item boundary divided.
        if next.continuation {
            return false;
        }
        // The character before the cluster's last, which a hyphen's rule
        // reads, found only where it does.
        let before = self.text.get(..next.start).unwrap_or_default();
        let ascii =
            next.item
                .key
                .ascii_answer(|| before.chars().rev().nth(1), pending.last, next.first);
        let breaks = match ascii {
            Some(breaks) => breaks,
            None => self
                .stream
                .as_mut()
                .is_some_and(|stream| stream.at(next.start)),
        };
        self.filter_break(&pending.item, pending.last, class, next, breaks)
    }

    /// Starts the independent stream for the base or annotation beginning
    /// here. Each part's items are looked ahead once, so dictionary context
    /// cannot extend into the next part. Style seams resume this stream as
    /// they resume the ordinary paragraph stream.
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
            at,
            self.cx.line(self.item.key),
            self.item.key,
        ));
    }

    /// The same rules as `breaks_between`, over the current ruby part's
    /// independent input. Never used at a part's end or inside a unit.
    #[inline(never)]
    fn inner_breaks_between(
        &mut self,
        pending: &Pending,
        class: ClusterClass,
        next: &Pending,
    ) -> bool {
        if next.continuation {
            return false;
        }
        let before = self
            .text
            .get(self.inner_start..next.start)
            .unwrap_or_default();
        let ascii =
            next.item
                .key
                .ascii_answer(|| before.chars().rev().nth(1), pending.last, next.first);
        let breaks = match ascii {
            Some(breaks) => breaks,
            None => self
                .inner_stream
                .as_mut()
                .is_some_and(|stream| stream.at(next.start)),
        };
        self.filter_break(&pending.item, pending.last, class, next, breaks)
    }

    /// Applies the filters to `breaks`, the line segmenter's answer at the
    /// boundary between a cluster of `class` in `item`, whose last character
    /// is `last`, and `next`.
    fn filter_break(
        &self,
        item: &ItemInfo,
        last: char,
        class: ClusterClass,
        next: &Pending,
        mut breaks: bool,
    ) -> bool {
        // Where the config says so, a small kana or the prolonged sound mark
        // is held to what precedes it under `line-break: normal`, as CSS
        // Text 3 now has it (csswg-drafts#10363). Line_Break CJ is taken as
        // NS, as `strict` takes it. UAX #14 still lets NS start a line after
        // a space or a zero width space (LB18, LB8) and nowhere else; a tab
        // and an ideographic space are BA. ICU, and Chrome with it, takes CJ
        // as an ideograph under `normal`. This runs before `break-spaces`,
        // which breaks after every space it keeps whatever follows.
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
        // `break-spaces`: after every preserved space and tab, between two
        // of them too; and after every other space separator that breaks,
        // as Chrome breaks between two ideographic spaces there, where
        // UAX #14 would not (WPT `break-spaces-with-ideographic-space-001`).
        if item.collapse == WhiteSpaceCollapse::BreakSpaces && class.is_breaking_space() {
            breaks = true;
        }
        // `hyphens: none`: a soft hyphen is no opportunity.
        if class == ClusterClass::SoftHyphen && item.hyphens_none {
            breaks = false;
        }
        // An atomic inline has an opportunity on either side, even where the
        // character beside it would suppress one. It has none beside GL other
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
        // An opportunity a space or a tab makes, which disappears or hangs at
        // the break, follows whether the box directly holding it wraps. One
        // between two other characters follows their nearest common ancestor
        // (CSS Text 3, section 5.1). Chrome decides a space's and a tab's so,
        // and an other space separator's as any character's.
        let wraps = if class.is_space_or_tab() {
            item.wraps
        } else {
            self.wraps_between(item, &next.item)
        };
        breaks && wraps
    }

    /// Whether text wraps at a boundary between `before` and `after`, where
    /// `before` is no space or tab.
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
        // At most one step per node, whatever the parents say.
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

    /// Ends the paragraph after the last cluster written. Kept out of line
    /// ([`open_paragraph`](Self::open_paragraph)).
    #[inline(never)]
    fn end_paragraph(&mut self) {
        let paragraph = &self.paragraph;
        let mut flags = paragraph.flags;
        if paragraph.east_asian {
            flags.insert(ParagraphFlags::HAS_EAST_ASIAN);
        }
        // Its base level, and where its clusters' levels change where some
        // differ from it. Resolved only where something right to left or a
        // control can raise a level, or the block reads right to left.
        let end = self.out.clusters.end_id();
        let (level, level_flags) = levels::resolve(
            self.content,
            self.out,
            &mut self.scratch.bidi,
            paragraph.start..end,
            paragraph.first_item,
            self.requested,
            paragraph.controls || paragraph.classes & LEVEL_CLASSES != 0,
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
        // Every cluster is written: the last item's end there.
        self.out.item_clusters.end = self.out.clusters.end_id();
        // The paragraph still open ends. Where there is none -- the text is
        // empty, or ends in a separator -- an empty one does, which is where
        // the caret sits after a final `<br>`.
        if !self.paragraph.open {
            self.open_paragraph();
        }
        self.paragraph.controls |= self.controls;
        self.end_paragraph();
        self.runs.finish(&mut self.scratch.runs);
        scripts::emit(
            &self.scratch.runs,
            &self.out.paragraphs,
            &self.scratch.bidi.changes,
            self.out.clusters.end_id(),
            &mut self.out.runs,
        );
        (self.out).set_control_levels(&self.scratch.bidi.control_levels);
    }
}
