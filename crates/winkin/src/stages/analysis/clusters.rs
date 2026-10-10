//! Stored clusters from text analysis.

use core::ops::Range;

use crate::data::{BitTable, Id, IdRange, Table, TextOffset, heap_bytes};
use crate::stages::content::VariantText;
use crate::unicode::{self, GraphemeClusterBreak};
use crate::work;

use super::{ClusterId, ParagraphFlags};

/// Where a cluster ends, whether it continues the one before, and whether it
/// holds a variation selector: 4 bytes.
///
/// The end byte shifted up two, with the continuation in bit 1 and the
/// selector in bit 0. The text holds under 2^30 bytes, so the end fits. A
/// cluster's start is the previous one's end.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ClusterEnd(u32);

impl ClusterEnd {
    const CONTINUATION: u32 = 1 << 1;
    const SELECTOR: u32 = 1 << 0;

    fn new(end: TextOffset, continuation: bool, selector: bool) -> Self {
        // `TextOffset::MAX` is under 2^30, so this cannot saturate.
        let shifted = u32::try_from(end.get() << 2).unwrap_or(!3);
        let continuation = if continuation { Self::CONTINUATION } else { 0 };
        let selector = if selector { Self::SELECTOR } else { 0 };
        Self(shifted | continuation | selector)
    }

    /// The byte the cluster ends at.
    pub(crate) fn end(self) -> TextOffset {
        // A `u32` shifted down two is always a position the text can hold.
        TextOffset::new(usize::try_from(self.0 >> 2).unwrap_or(TextOffset::MAX))
    }

    /// Whether the cluster is the later part of a grapheme that an item
    /// boundary divided: [`Clusters::is_continuation`] asks it.
    pub(super) fn is_continuation(self) -> bool {
        self.0 & Self::CONTINUATION != 0
    }

    /// Whether a character of the cluster after its first is a variation
    /// selector, VS1 to VS256.
    ///
    /// Font selection reads a presentation from it, or asks a font for the
    /// pair. A run of text in one font cannot continue across one. The bit
    /// is gathered with the cluster, so a cluster without one costs
    /// selection nothing.
    pub(crate) fn is_variation_selector(self) -> bool {
        self.0 & Self::SELECTOR != 0
    }
}

/// What a cluster is, from its first character and the item it is in.
///
/// The classes the later stages branch on: font selection on the emoji
/// classes, shaping on the classes it does not shape, measuring and breaking
/// on the spaces, and every stage on separators and objects.
///
/// Numbered as a cluster's attributes hold it, in their low four bits
/// ([`ClusterAttrs::CLASSES`]).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub(crate) enum ClusterClass {
    /// Anything else.
    Text = 0,
    /// Emoji presentation: an Emoji_Presentation character, or a variation
    /// selector 16, keycap, flag, modifier, tag or ZWJ sequence.
    Emoji = 1,
    /// An Extended_Pictographic character in text presentation.
    Symbol = 2,
    /// U+0020.
    Space = 3,
    /// A space that does not break: U+00A0, U+2007, U+202F. It never hangs.
    NoBreakSpace = 4,
    /// U+0009.
    Tab = 5,
    /// The other space separators: U+1680, U+2000 to U+200A but U+2007,
    /// U+205F, U+3000.
    OtherSpace = 6,
    /// A paragraph separator that draws nothing: LF, CRLF, U+2028, U+2029,
    /// and the `\n` of a `<br>`. A must-break is this class or
    /// [`DrawnSeparator`](Self::DrawnSeparator), and nothing else
    /// ([`is_forced_break`](Self::is_forced_break)).
    Separator = 7,
    /// A paragraph separator drawn as a glyph: VT, FF or NEL.
    ///
    /// These are the control characters (Cc) among UAX #14's mandatory
    /// breaks. CSS Text 3 renders them visibly and has them force a line
    /// break. Each is shaped in the font that covers it, as any symbol is. It
    /// takes room on its line rather than hanging, since it is drawn.
    DrawnSeparator = 13,
    /// An atomic inline's U+FFFC. A U+FFFC the caller wrote in its text is
    /// text.
    Object = 8,
    /// U+00AD SOFT HYPHEN.
    SoftHyphen = 9,
    /// U+200B the caller wrote. It is shaped with the text around it, which
    /// hides it, as Chrome keeps it in its text item.
    ZeroWidthSpace = 10,
    /// Another default-ignorable character standing alone: a bidi control,
    /// a word joiner, a lone joiner or variation selector. Shaped with the
    /// text around it, as [`ZeroWidthSpace`](Self::ZeroWidthSpace) is.
    Control = 11,
    /// U+200B the builder generated: a `<wbr>`'s, or the wrap opportunity
    /// collapsing kept.
    ///
    /// Each is an item of its own flagged
    /// [`GENERATED`](crate::stages::content::ItemFlags::GENERATED). It is a break
    /// opportunity and nothing else. It ends any shaping run it sits in, as
    /// Blink's control item for it does. Its map unit is `Generated`.
    BreakOpportunity = 12,
}

impl ClusterClass {
    /// The flag a cluster of the class raises in its paragraph's: a tab or a
    /// soft hyphen, and an internal shaping stop; none for the rest.
    pub(super) fn paragraph_flags(self) -> ParagraphFlags {
        match self {
            Self::Tab => ParagraphFlags::HAS_TABS.union(ParagraphFlags::HAS_SHAPING_STOPS),
            Self::Object | Self::BreakOpportunity => ParagraphFlags::HAS_SHAPING_STOPS,
            Self::SoftHyphen => ParagraphFlags::HAS_SOFT_HYPHEN,
            _ => ParagraphFlags::NONE,
        }
    }

    /// Whether a cluster of the class forces a line break after it and ends
    /// its paragraph: a separator, drawn or not.
    #[inline]
    pub(crate) fn is_forced_break(self) -> bool {
        matches!(self, Self::Separator | Self::DrawnSeparator)
    }

    /// Whether a cluster of the class is shaped.
    ///
    /// Four classes have no glyphs: a tab, whose advance is the line's; a
    /// separator that draws nothing; an atomic inline's U+FFFC, whose advance
    /// is its box's; and a U+200B the builder generated. Each ends any
    /// shaping run it sits in, as Blink ends a shaping run at its control
    /// items, which these are. A separator that is a control character is
    /// drawn, as CSS has it, and shaped as the last cluster of its paragraph.
    ///
    /// A default-ignorable the caller wrote -- a soft hyphen, a U+200B, a
    /// word joiner, a bidi control -- is shaped with the text around it. Blink
    /// keeps it in its text item too, and the shaper hides it. So a ligature
    /// or a kern across one survives on an unbroken line (measured in Chrome
    /// 153: `AV` in Arial kerns across each). A line that breaks there
    /// reshapes its end where the font shaped across it. It draws a soft
    /// hyphen's hyphen from generated text.
    pub(crate) fn is_shaped(self) -> bool {
        !matches!(
            self,
            Self::Tab | Self::Separator | Self::Object | Self::BreakOpportunity
        )
    }

    /// Whether a cluster of the class is a space or a tab.
    ///
    /// A line justified at its spaces puts room after these. Chrome does not
    /// reshape a line's end at a break that follows one.
    #[inline]
    pub(crate) fn is_space_or_tab(self) -> bool {
        matches!(self, Self::Space | Self::Tab)
    }

    /// Whether a cluster of the class is a space that breaks.
    ///
    /// These are a space, a tab or another space separator, never a no-break
    /// space. Such a cluster hangs at a line's end unless `break-spaces`
    /// makes it content, and `break-spaces` breaks after it. Bidi sets the
    /// white space a line ends with at its paragraph's level.
    #[inline]
    pub(crate) fn is_breaking_space(self) -> bool {
        matches!(self, Self::Space | Self::Tab | Self::OtherSpace)
    }
}

/// A cluster's class and the decisions about its end, in 1 byte.
///
/// | Bits | Field |
/// |---|---|
/// | 0-3 | [`ClusterClass`] |
/// | 4 | [`BREAK_AFTER`](Self::BREAK_AFTER) |
/// | 5 | [`EMERGENCY_AFTER`](Self::EMERGENCY_AFTER) |
/// | 6 | [`HANGS`](Self::HANGS) |
/// | 7 | [`SHAPE_BREAK_AFTER`](Self::SHAPE_BREAK_AFTER) |
///
/// It holds decisions, never measurements: nothing a `::first-line` style can
/// change is here.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ClusterAttrs(u8);

impl ClusterAttrs {
    const CLASS: u8 = 0x0F;

    /// Each class by its number, the four bits the attributes hold it in,
    /// and text where a number names none.
    const CLASSES: [ClusterClass; 16] = {
        use ClusterClass::*;
        [
            Text,
            Emoji,
            Symbol,
            Space,
            NoBreakSpace,
            Tab,
            OtherSpace,
            Separator,
            Object,
            SoftHyphen,
            ZeroWidthSpace,
            Control,
            BreakOpportunity,
            DrawnSeparator,
            Text,
            Text,
        ]
    };

    /// A soft wrap opportunity at the cluster's end, after every filter.
    ///
    /// It is never set on a paragraph's last cluster, whose end is the
    /// paragraph's. Nor before a separator, which ends the line there anyway,
    /// nor before a continuation, inside a grapheme. Ruby parts share these
    /// facts; whole-column readers skip their interiors.
    pub(crate) const BREAK_AFTER: u8 = 1 << 4;
    /// An `overflow-wrap` break at the cluster's end, for when nothing else
    /// fits. The same three exceptions hold.
    pub(crate) const EMERGENCY_AFTER: u8 = 1 << 5;
    /// The cluster hangs when it ends a line.
    pub(crate) const HANGS: u8 = 1 << 6;
    /// A shaping break at the cluster's end, for a reason neither the runs
    /// nor the styles nor the classes record.
    pub(crate) const SHAPE_BREAK_AFTER: u8 = 1 << 7;

    pub(super) fn new(class: ClusterClass) -> Self {
        Self(class as u8)
    }

    pub(super) fn with(self, bit: u8, on: bool) -> Self {
        if on { Self(self.0 | bit) } else { self }
    }

    /// The cluster's class.
    pub(crate) fn class(self) -> ClusterClass {
        Self::CLASSES
            .get(usize::from(self.0 & Self::CLASS))
            .copied()
            .unwrap_or(ClusterClass::Text)
    }

    /// A copy with these facts masked for a narrower query context.
    pub(crate) fn without(self, bits: u8) -> Self {
        Self(self.0 & !bits)
    }

    /// Whether every bit of `bits` is set.
    pub(crate) fn has(self, bits: u8) -> bool {
        self.0 & bits == bits
    }
}

/// The clusters: 5 bytes and a bit each.
pub(crate) struct Clusters {
    pub(super) ends: Table<ClusterId, ClusterEnd>,
    pub(super) attrs: Table<ClusterId, ClusterAttrs>,
    /// One bit per cluster, set where the cluster has
    /// [`BREAK_AFTER`](ClusterAttrs::BREAK_AFTER). It is an index over
    /// `attrs`, written with it and never on its own. The breaker scans it
    /// back to the last opportunity 64 clusters a word.
    pub(super) stops: BitTable<ClusterId>,
}

impl Clusters {
    pub(super) const fn new() -> Self {
        Self {
            ends: Table::new(),
            attrs: Table::new(),
            stops: BitTable::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.ends.clear();
        self.attrs.clear();
        self.stops.clear();
    }

    /// Makes room for `count` clusters, the most the text can make: every
    /// cluster holds a character at least.
    pub(super) fn reserve(&mut self, count: usize) {
        self.ends.reserve(count);
        self.attrs.reserve(count);
        self.stops.reserve(count);
    }

    /// Appends a cluster ending at `end`.
    ///
    /// `None` only past what a `ClusterId` names, which a text capped at
    /// [`TextOffset::MAX`] bytes cannot reach.
    #[inline]
    pub(super) fn push(
        &mut self,
        end: TextOffset,
        continuation: bool,
        selector: bool,
        attrs: ClusterAttrs,
    ) -> Option<ClusterId> {
        let id = self
            .ends
            .push(ClusterEnd::new(end, continuation, selector))?;
        self.attrs
            .push_bounded(attrs, "the two columns have one length");
        self.stops.push(id, attrs.has(ClusterAttrs::BREAK_AFTER));
        Some(id)
    }

    /// How many clusters there are.
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether there are none.
    pub(crate) fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The position at the text's end: the id one past the last cluster,
    /// where every range of clusters reaching the end stops.
    pub(crate) fn end_id(&self) -> ClusterId {
        self.ends.next_id()
    }

    /// Every cluster's id, in text order, for tests to walk.
    #[cfg(test)]
    pub(super) fn ids(&self) -> impl DoubleEndedIterator<Item = ClusterId> + ExactSizeIterator {
        self.ends.ids()
    }

    /// Where `cluster` ends and what else its end says, or `None` past the
    /// last: for a walk that reads each cluster's end once, and takes the
    /// next one's start from it.
    pub(crate) fn end(&self, cluster: ClusterId) -> Option<ClusterEnd> {
        self.ends.get(cluster).copied()
    }

    /// Whether `cluster` is the later part of a grapheme that an item
    /// boundary divided; not past the last.
    ///
    /// Caret motion treats it and the cluster before as one stop. Font
    /// selection tries the earlier part's font first. A line never breaks
    /// before it.
    #[inline]
    pub(crate) fn is_continuation(&self, cluster: ClusterId) -> bool {
        self.ends
            .get(cluster)
            .is_some_and(|end| end.is_continuation())
    }

    /// Whether a character of `cluster` after its first is a variation
    /// selector ([`ClusterEnd::is_variation_selector`]); not past the last.
    #[inline]
    pub(crate) fn is_variation_selector(&self, cluster: ClusterId) -> bool {
        self.ends
            .get(cluster)
            .is_some_and(|end| end.is_variation_selector())
    }

    /// Whether `cluster` goes on with the grapheme the cluster before it
    /// started, `text` being the layout's.
    ///
    /// It does where it continues a grapheme an item boundary divided
    /// ([`is_continuation`](Self::is_continuation)), and where it is the
    /// marks after a space, which a line may break before. Carets and word
    /// bounds keep to the grapheme in both.
    pub(crate) fn continues_grapheme(&self, text: &str, cluster: ClusterId) -> bool {
        if self.is_continuation(cluster) {
            return true;
        }
        let start = self.start(cluster).get();
        let before = start.checked_sub(1).and_then(|at| text.as_bytes().get(at));
        before == Some(&b' ')
            && text
                .get(start..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|ch| {
                    matches!(
                        unicode::rare_props(ch).grapheme_cluster_break(),
                        GraphemeClusterBreak::Extend
                            | GraphemeClusterBreak::SpacingMark
                            | GraphemeClusterBreak::Zwj
                    )
                })
    }

    /// Where `cluster` starts: where the one before it ends.
    pub(crate) fn start(&self, cluster: ClusterId) -> TextOffset {
        match cluster.get().checked_sub(1) {
            Some(before) => self
                .ends
                .get(ClusterId::new(before))
                .map_or(TextOffset::default(), |end| end.end()),
            None => TextOffset::default(),
        }
    }

    /// The cluster holding byte `at` of the text, by halving: the first that
    /// ends past it, which is the one starting at it where `at` is a
    /// boundary; the position at the text's end for `at` there or past it.
    pub(crate) fn at(&self, pos: TextOffset) -> ClusterId {
        work::seek();
        ClusterId::new(self.ends.as_slice().partition_point(|end| end.end() <= pos))
    }

    /// The text `cluster` covers, or an empty range past the last.
    pub(crate) fn range(&self, cluster: ClusterId) -> Range<TextOffset> {
        let start = self.start(cluster);
        start..self.end(cluster).map_or(start, ClusterEnd::end)
    }

    /// `cluster`'s text in `source`, the content's or the first line's as
    /// a variant reads it: nothing past the text.
    #[inline]
    pub(crate) fn text<'t>(&self, source: VariantText<'t>, cluster: ClusterId) -> &'t str {
        source.slice(self.range(cluster))
    }

    /// The first character of `cluster`'s text in `source`, what a question
    /// of the character a cluster starts with reads (a mark trimmed or
    /// hanging, kana beside ruby, an emphasis mark skipped), or `None` past
    /// the text.
    #[inline]
    pub(crate) fn first_char(&self, source: VariantText<'_>, cluster: ClusterId) -> Option<char> {
        self.text(source, cluster).chars().next()
    }

    /// The last character of `cluster`'s text in `source`, or `None` past
    /// the text.
    #[inline]
    pub(crate) fn last_char(&self, source: VariantText<'_>, cluster: ClusterId) -> Option<char> {
        self.text(source, cluster).chars().next_back()
    }

    /// Whether `cluster` is a tab: the one cluster whose width is not a fact
    /// of the text. The breaker, line layout and the intrinsic sizes size it
    /// where it lands.
    #[inline]
    pub(crate) fn is_tab(&self, cluster: ClusterId) -> bool {
        self.class(cluster) == Some(ClusterClass::Tab)
    }

    /// `cluster`'s class and decisions, or `None` past the last.
    pub(crate) fn attrs(&self, cluster: ClusterId) -> Option<ClusterAttrs> {
        self.attrs.get(cluster).copied()
    }

    /// `cluster`'s class, or `None` past the last.
    #[inline]
    pub(crate) fn class(&self, cluster: ClusterId) -> Option<ClusterClass> {
        self.attrs(cluster).map(ClusterAttrs::class)
    }

    /// Whether boundary `boundary` follows a forced break.
    pub(crate) fn follows_forced_break(&self, boundary: ClusterId) -> bool {
        boundary.get() > 0
            && self
                .class(ClusterId::new(boundary.get() - 1))
                .is_some_and(ClusterClass::is_forced_break)
    }

    // The questions of one cluster below are inline, as `is_tab` is: the
    // breaker asks them of every line, and out of line they cost the
    // relayout of a short paragraph a few percent (parley's japanese-20, 4%).

    /// Whether `cluster` hangs when it ends a line
    /// ([`HANGS`](ClusterAttrs::HANGS)); not past the last.
    #[inline]
    pub(crate) fn hangs(&self, cluster: ClusterId) -> bool {
        self.attrs(cluster)
            .is_some_and(|attrs| attrs.has(ClusterAttrs::HANGS))
    }

    /// Whether `cluster` is white space that breaks
    /// ([`ClusterClass::is_breaking_space`]): a space, a tab or an other
    /// space separator, and not the separator or a generated opportunity
    /// that hang with it.
    #[inline]
    pub(crate) fn is_breaking_space(&self, cluster: ClusterId) -> bool {
        self.class(cluster)
            .is_some_and(ClusterClass::is_breaking_space)
    }

    /// Whether the cluster before the boundary `end` is a space or a tab.
    ///
    /// Chrome does not reshape a line's end at a break after one. It is false
    /// at the text's start.
    #[inline]
    pub(crate) fn space_or_tab_before(&self, end: ClusterId) -> bool {
        end.get()
            .checked_sub(1)
            .and_then(|last| self.class(ClusterId::new(last)))
            .is_some_and(ClusterClass::is_space_or_tab)
    }

    /// Whether every one of `clusters` is white space that breaks, or the
    /// separator: spaces that end the line before them rather than make one
    /// of their own.
    pub(crate) fn only_spaces(&self, clusters: Range<ClusterId>) -> bool {
        clusters.ids().all(|cluster| {
            self.class(cluster)
                .is_some_and(|class| class.is_breaking_space() || class == ClusterClass::Separator)
        })
    }

    /// How many of `clusters` are spaces or tabs
    /// ([`ClusterClass::is_space_or_tab`]), in one walk over their attrs;
    /// none where the range is not the text's.
    pub(crate) fn spaces_or_tabs(&self, clusters: Range<ClusterId>) -> usize {
        self.attrs
            .get_slice(clusters)
            .unwrap_or_default()
            .iter()
            .filter(|attrs| attrs.class().is_space_or_tab())
            .count()
    }

    /// The boundary after the last of `clusters` with a soft wrap
    /// opportunity after it ([`BREAK_AFTER`](ClusterAttrs::BREAK_AFTER)),
    /// where a line may end: found 64 clusters at a time.
    pub(crate) fn last_opportunity(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        self.stops
            .last(clusters)
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// The boundary after the first of `clusters` with a soft wrap
    /// opportunity after it, found 64 clusters at a time.
    pub(crate) fn first_opportunity(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        self.stops
            .first(clusters)
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// The boundary after the last of `clusters` with an `overflow-wrap`
    /// break after it ([`EMERGENCY_AFTER`](ClusterAttrs::EMERGENCY_AFTER)),
    /// walking back from their end.
    pub(crate) fn last_emergency(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        let mut at = clusters.end;
        while at > clusters.start {
            at = ClusterId::new(at.get() - 1);
            if self
                .attrs(at)
                .is_some_and(|attrs| attrs.has(ClusterAttrs::EMERGENCY_AFTER))
            {
                return Some(ClusterId::new(at.get() + 1));
            }
        }
        None
    }

    /// The boundary after the first of `clusters` with an `overflow-wrap`
    /// break after it, walking on from their start.
    pub(crate) fn first_emergency(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        clusters
            .ids()
            .find(|&cluster| {
                self.attrs(cluster)
                    .is_some_and(|attrs| attrs.has(ClusterAttrs::EMERGENCY_AFTER))
            })
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// The bytes the opportunities' index takes, which the tests count
    /// against the design's budget.
    #[cfg(test)]
    pub(crate) fn stop_bytes(&self) -> usize {
        self.stops.bytes()
    }
}

heap_bytes! {
    Clusters { ends, attrs, stops }
}
