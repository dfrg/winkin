//! The cluster table: each cluster's end, class and break decisions.

use core::ops::Range;

use crate::data::{BitTable, Id, IdRange, Table, TextOffset, heap_bytes};
use crate::stages::content::VariantText;
use crate::unicode::{self, GraphemeClusterBreak};
use crate::work;

use super::{ClusterId, ParagraphFlags};

/// A cluster's end byte, and whether it continues the cluster before and
/// holds a variation selector, in 4 bytes.
///
/// Bits 2 and up hold the end, bit 1 the continuation and bit 0 the
/// selector. A text is under 2^30 bytes, so the end fits. A cluster starts
/// where the previous one ends.
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

    /// Returns the byte the cluster ends at.
    pub(crate) fn end(self) -> TextOffset {
        // A `u32` shifted down two always fits a `TextOffset`.
        TextOffset::new(usize::try_from(self.0 >> 2).unwrap_or(TextOffset::MAX))
    }

    /// Returns whether the cluster is the later part of a grapheme split by
    /// an item boundary.
    pub(super) fn is_continuation(self) -> bool {
        self.0 & Self::CONTINUATION != 0
    }

    /// Returns whether a character after the cluster's first is a variation
    /// selector, VS1 to VS256.
    ///
    /// Font selection reads a presentation from it or asks a font for the
    /// pair, and a run in one font does not continue across it. Clusters
    /// without one cost selection nothing.
    pub(crate) fn is_variation_selector(self) -> bool {
        self.0 & Self::SELECTOR != 0
    }
}

/// A cluster's kind, from its first character and its item.
///
/// Font selection branches on the emoji classes, shaping on the unshaped
/// classes, measurement and breaking on the spaces, and every stage on
/// separators and objects. The discriminant is the low four bits of
/// [`ClusterAttrs`].
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
    /// and the `\n` of a `<br>`.
    ///
    /// Only this class and [`DrawnSeparator`](Self::DrawnSeparator) force a
    /// break ([`is_forced_break`](Self::is_forced_break)).
    Separator = 7,
    /// A paragraph separator drawn as a glyph: VT, FF or NEL.
    ///
    /// These are the control characters (Cc) among UAX #14's mandatory
    /// breaks. CSS Text 3 renders them visibly and has them force a line
    /// break. Each is shaped in a font that covers it, like any symbol, and
    /// takes room on its line instead of hanging.
    DrawnSeparator = 13,
    /// An atomic inline's U+FFFC. A U+FFFC in the input text is
    /// [`Text`](Self::Text).
    Object = 8,
    /// U+00AD SOFT HYPHEN.
    SoftHyphen = 9,
    /// A U+200B in the input text. It is shaped with the text around it,
    /// which hides it, as Chrome keeps it in its text item.
    ZeroWidthSpace = 10,
    /// Another default-ignorable character standing alone: a bidi control,
    /// a word joiner, a lone joiner or variation selector. It is shaped with
    /// the text around it, like [`ZeroWidthSpace`](Self::ZeroWidthSpace).
    Control = 11,
    /// A U+200B the builder generated: a `<wbr>`, or a wrap opportunity that
    /// white-space collapsing keeps.
    ///
    /// Each is its own item, flagged
    /// [`GENERATED`](crate::stages::content::ItemFlags::GENERATED), and is
    /// only a break opportunity. It ends any shaping run it sits in, as
    /// Blink's control item does. Its map unit is `Generated`.
    BreakOpportunity = 12,
}

impl ClusterClass {
    /// Returns the flags a cluster of this class sets on its paragraph.
    pub(super) fn paragraph_flags(self) -> ParagraphFlags {
        match self {
            Self::Tab => ParagraphFlags::HAS_TABS.union(ParagraphFlags::HAS_SHAPING_STOPS),
            Self::Object | Self::BreakOpportunity => ParagraphFlags::HAS_SHAPING_STOPS,
            Self::SoftHyphen => ParagraphFlags::HAS_SOFT_HYPHEN,
            _ => ParagraphFlags::NONE,
        }
    }

    /// Returns whether the class forces a line break after it and ends its
    /// paragraph: a separator, drawn or not.
    #[inline]
    pub(crate) fn is_forced_break(self) -> bool {
        matches!(self, Self::Separator | Self::DrawnSeparator)
    }

    /// Returns whether the class is shaped.
    ///
    /// Four classes have no glyphs: a tab, sized by the line; a separator
    /// that draws nothing; an atomic inline's U+FFFC, sized by its box; and
    /// a generated U+200B. Each ends any shaping run it sits in, as Blink's
    /// control items do. A drawn separator is shaped as its paragraph's last
    /// cluster.
    ///
    /// A default-ignorable in the input text (a soft hyphen, U+200B, a word
    /// joiner, a bidi control) is shaped with the text around it, which hides
    /// it, as Blink keeps it in its text item. A ligature or kern across one
    /// survives on an unbroken line (Chrome 153: `AV` in Arial kerns across
    /// each). A line that breaks there reshapes its end, and draws a soft
    /// hyphen's hyphen from generated text.
    pub(crate) fn is_shaped(self) -> bool {
        !matches!(
            self,
            Self::Tab | Self::Separator | Self::Object | Self::BreakOpportunity
        )
    }

    /// Returns whether the class is a space or a tab.
    ///
    /// Justification adds room after these.
    #[inline]
    pub(crate) fn is_space_or_tab(self) -> bool {
        matches!(self, Self::Space | Self::Tab)
    }

    /// Returns whether the class is a breaking space: a space, a tab or
    /// another space separator, never a no-break space.
    ///
    /// Such a cluster hangs at a line's end unless `break-spaces` makes it
    /// content and breaks after it. Bidi sets trailing white space at the
    /// paragraph's level.
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
/// It holds decisions, never measurements, so no `::first-line` style changes
/// it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ClusterAttrs(u8);

impl ClusterAttrs {
    const CLASS: u8 = 0x0F;

    /// Each class by its number, with [`ClusterClass::Text`] for unused
    /// numbers.
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
    /// Never set on a paragraph's last cluster, before a separator (which
    /// ends the line anyway), or before a continuation. Ruby parts carry it
    /// too; readers of whole columns skip their interiors.
    pub(crate) const BREAK_AFTER: u8 = 1 << 4;
    /// An `overflow-wrap` break at the cluster's end, used when nothing else
    /// fits. The same three exceptions hold.
    pub(crate) const EMERGENCY_AFTER: u8 = 1 << 5;
    /// The cluster hangs when it ends a line.
    pub(crate) const HANGS: u8 = 1 << 6;
    /// A shaping break at the cluster's end that the runs, styles and
    /// classes do not already imply.
    pub(crate) const SHAPE_BREAK_AFTER: u8 = 1 << 7;

    pub(super) fn new(class: ClusterClass) -> Self {
        Self(class as u8)
    }

    pub(super) fn with(self, bit: u8, on: bool) -> Self {
        if on { Self(self.0 | bit) } else { self }
    }

    /// Returns the cluster's class.
    pub(crate) fn class(self) -> ClusterClass {
        Self::CLASSES
            .get(usize::from(self.0 & Self::CLASS))
            .copied()
            .unwrap_or(ClusterClass::Text)
    }

    /// Returns these attributes with `bits` cleared.
    pub(crate) fn without(self, bits: u8) -> Self {
        Self(self.0 & !bits)
    }

    /// Returns whether every bit of `bits` is set.
    pub(crate) fn has(self, bits: u8) -> bool {
        self.0 & bits == bits
    }
}

/// The clusters, at 5 bytes and a bit each.
pub(crate) struct Clusters {
    pub(super) ends: Table<ClusterId, ClusterEnd>,
    pub(super) attrs: Table<ClusterId, ClusterAttrs>,
    /// One bit per cluster, set where it has
    /// [`BREAK_AFTER`](ClusterAttrs::BREAK_AFTER). It indexes `attrs` and is
    /// written with it. The breaker scans it 64 clusters per word.
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

    /// Makes room for `count` clusters.
    pub(super) fn reserve(&mut self, count: usize) {
        self.ends.reserve(count);
        self.attrs.reserve(count);
        self.stops.reserve(count);
    }

    /// Appends a cluster ending at `end`.
    ///
    /// Returns `None` past the last `ClusterId`, which a text of at most
    /// [`TextOffset::MAX`] bytes never reaches.
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

    /// Returns the number of clusters.
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    /// Returns whether there are no clusters.
    pub(crate) fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Returns the boundary at the text's end, one past the last cluster.
    pub(crate) fn end_id(&self) -> ClusterId {
        self.ends.next_id()
    }

    /// Returns every cluster's id, in text order.
    #[cfg(test)]
    pub(super) fn ids(&self) -> impl DoubleEndedIterator<Item = ClusterId> + ExactSizeIterator {
        self.ends.ids()
    }

    /// Returns `cluster`'s end, or `None` past the last.
    ///
    /// A forward walk reads each end once and takes the next cluster's start
    /// from it.
    pub(crate) fn end(&self, cluster: ClusterId) -> Option<ClusterEnd> {
        self.ends.get(cluster).copied()
    }

    /// Returns whether `cluster` is the later part of a grapheme split by an
    /// item boundary; false past the last.
    ///
    /// Caret motion treats it and the cluster before as one stop, font
    /// selection tries the earlier part's font first, and no line breaks
    /// before it.
    #[inline]
    pub(crate) fn is_continuation(&self, cluster: ClusterId) -> bool {
        self.ends
            .get(cluster)
            .is_some_and(|end| end.is_continuation())
    }

    /// Returns whether `cluster` continues the grapheme of the cluster
    /// before, in the layout's `text`.
    ///
    /// True for a [continuation](Self::is_continuation) and for marks after a
    /// space, which a line may break before. Carets and word bounds treat
    /// both as one grapheme.
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

    /// Returns where `cluster` starts, the end of the cluster before.
    pub(crate) fn start(&self, cluster: ClusterId) -> TextOffset {
        match cluster.get().checked_sub(1) {
            Some(before) => self
                .ends
                .get(ClusterId::new(before))
                .map_or(TextOffset::default(), |end| end.end()),
            None => TextOffset::default(),
        }
    }

    /// Returns the cluster holding byte `pos`, by binary search.
    ///
    /// At a boundary that is the cluster starting there. At or past the
    /// text's end it is [`end_id`](Self::end_id).
    pub(crate) fn at(&self, pos: TextOffset) -> ClusterId {
        work::seek();
        ClusterId::new(self.ends.as_slice().partition_point(|end| end.end() <= pos))
    }

    /// Returns the cluster holding byte `at`, as [`at`](Self::at) does, by
    /// stepping from `near`.
    ///
    /// Costs one step per cluster between and no search, so `near` should
    /// be close.
    pub(crate) fn walk_to(&self, near: ClusterId, at: TextOffset) -> ClusterId {
        let end = self.end_id();
        let mut cluster = near.min(end);
        // Steps back to the cluster starting at or before `at`.
        while cluster.get() > 0 && self.start(cluster) > at {
            work::step();
            cluster = ClusterId::new(cluster.get() - 1);
        }
        // Steps on to the first cluster ending past it.
        while cluster < end && self.range(cluster).end <= at {
            work::step();
            cluster = ClusterId::new(cluster.get() + 1);
        }
        cluster
    }

    /// Returns the bytes `cluster` covers, or an empty range past the last.
    pub(crate) fn range(&self, cluster: ClusterId) -> Range<TextOffset> {
        let start = self.start(cluster);
        start..self.end(cluster).map_or(start, ClusterEnd::end)
    }

    /// Returns `cluster`'s text in `source`, the content's or the first
    /// line's; empty past the text.
    #[inline]
    pub(crate) fn text<'t>(&self, source: VariantText<'t>, cluster: ClusterId) -> &'t str {
        source.slice(self.range(cluster))
    }

    /// Returns the first character of `cluster`'s text in `source`, or
    /// `None` past the text.
    #[inline]
    pub(crate) fn first_char(&self, source: VariantText<'_>, cluster: ClusterId) -> Option<char> {
        self.text(source, cluster).chars().next()
    }

    /// Returns the last character of `cluster`'s text in `source`, or `None`
    /// past the text.
    #[inline]
    pub(crate) fn last_char(&self, source: VariantText<'_>, cluster: ClusterId) -> Option<char> {
        self.text(source, cluster).chars().next_back()
    }

    /// Returns whether `cluster` is a tab.
    ///
    /// A tab's width depends on where it lands, so the breaker, line layout
    /// and the intrinsic sizes compute it there.
    #[inline]
    pub(crate) fn is_tab(&self, cluster: ClusterId) -> bool {
        self.class(cluster) == Some(ClusterClass::Tab)
    }

    /// Returns `cluster`'s attributes, or `None` past the last.
    pub(crate) fn attrs(&self, cluster: ClusterId) -> Option<ClusterAttrs> {
        self.attrs.get(cluster).copied()
    }

    /// Returns `cluster`'s class, or `None` past the last.
    #[inline]
    pub(crate) fn class(&self, cluster: ClusterId) -> Option<ClusterClass> {
        self.attrs(cluster).map(ClusterAttrs::class)
    }

    /// Returns the end of `clusters` without the trailing undrawn separators
    /// and generated break opportunities.
    pub(crate) fn drawn_end(&self, clusters: Range<ClusterId>) -> ClusterId {
        let mut end = clusters.end;
        while end > clusters.start {
            let last = ClusterId::new(end.get() - 1);
            match self.class(last) {
                Some(ClusterClass::Separator | ClusterClass::BreakOpportunity) => end = last,
                _ => break,
            }
        }
        end
    }

    /// Returns whether `boundary` follows a forced break.
    pub(crate) fn follows_forced_break(&self, boundary: ClusterId) -> bool {
        boundary.get() > 0
            && self
                .class(ClusterId::new(boundary.get() - 1))
                .is_some_and(ClusterClass::is_forced_break)
    }

    // The per-cluster queries below are inline, like `is_tab`: the breaker
    // calls them on every line, and out of line they slow a short
    // paragraph's relayout by a few percent (parley's japanese-20, 4%).

    /// Returns whether `cluster` hangs when it ends a line
    /// ([`HANGS`](ClusterAttrs::HANGS)); false past the last.
    #[inline]
    pub(crate) fn hangs(&self, cluster: ClusterId) -> bool {
        self.attrs(cluster)
            .is_some_and(|attrs| attrs.has(ClusterAttrs::HANGS))
    }

    /// Returns whether `cluster` is a breaking space
    /// ([`ClusterClass::is_breaking_space`]). Separators and generated
    /// opportunities, which hang with spaces, are not.
    #[inline]
    pub(crate) fn is_breaking_space(&self, cluster: ClusterId) -> bool {
        self.class(cluster)
            .is_some_and(ClusterClass::is_breaking_space)
    }

    /// Returns whether the cluster before boundary `end` is a space or a
    /// tab; false at the text's start.
    ///
    /// Chrome reshapes a line's end at a break after one only in a few
    /// cases, which line layout tests apart.
    #[inline]
    pub(crate) fn space_or_tab_before(&self, end: ClusterId) -> bool {
        end.get()
            .checked_sub(1)
            .and_then(|last| self.class(ClusterId::new(last)))
            .is_some_and(ClusterClass::is_space_or_tab)
    }

    /// Returns whether every one of `clusters` is a breaking space or an
    /// undrawn separator, which end the line before them instead of making
    /// one of their own.
    pub(crate) fn only_spaces(&self, clusters: Range<ClusterId>) -> bool {
        clusters.ids().all(|cluster| {
            self.class(cluster)
                .is_some_and(|class| class.is_breaking_space() || class == ClusterClass::Separator)
        })
    }

    /// Returns how many of `clusters` are spaces or tabs
    /// ([`ClusterClass::is_space_or_tab`]); 0 for a range outside the text.
    pub(crate) fn spaces_or_tabs(&self, clusters: Range<ClusterId>) -> usize {
        self.attrs
            .get_slice(clusters)
            .unwrap_or_default()
            .iter()
            .filter(|attrs| attrs.class().is_space_or_tab())
            .count()
    }

    /// Returns the boundary after the last of `clusters` with a soft wrap
    /// opportunity ([`BREAK_AFTER`](ClusterAttrs::BREAK_AFTER)), scanning 64
    /// clusters at a time.
    pub(crate) fn last_opportunity(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        self.stops
            .last(clusters)
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// Returns the boundary after the first of `clusters` with a soft wrap
    /// opportunity, scanning 64 clusters at a time.
    pub(crate) fn first_opportunity(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        self.stops
            .first(clusters)
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// Returns the boundary after the last of `clusters` with an
    /// `overflow-wrap` break
    /// ([`EMERGENCY_AFTER`](ClusterAttrs::EMERGENCY_AFTER)).
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

    /// Returns the boundary after the first of `clusters` with an
    /// `overflow-wrap` break.
    pub(crate) fn first_emergency(&self, clusters: Range<ClusterId>) -> Option<ClusterId> {
        clusters
            .ids()
            .find(|&cluster| {
                self.attrs(cluster)
                    .is_some_and(|attrs| attrs.has(ClusterAttrs::EMERGENCY_AFTER))
            })
            .map(|cluster| ClusterId::new(cluster.get() + 1))
    }

    /// Returns the bytes the opportunity bits take, excluding spare capacity.
    #[cfg(test)]
    pub(crate) fn stop_bytes(&self) -> usize {
        self.stops.bytes()
    }
}

heap_bytes! {
    Clusters { ends, attrs, stops }
}
