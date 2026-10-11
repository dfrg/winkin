//! `text-autospace`: the room added where an ideograph meets a narrow letter
//! or digit, as Chrome's `TextAutoSpace::Apply` adds it.
//!
//! The room is an eighth of the ideograph advance and goes after the cluster
//! before the seam. A seam reaches across items and box edges. An atomic
//! inline, a float, a forced break or a `unicode-bidi` box ends it, since
//! Chrome writes each as a character. A line ending at a seam gives the room
//! back (`UnapplyAutoSpacing`), even where Chrome keeps it after a
//! right-to-left item.
//!
//! Where the side before a seam reads right to left and the side after reads
//! left to right, Chrome writes the room before the later item's first
//! glyph. The prefix still holds it before the boundary. Line layout draws it
//! with the later text instead ([`AutospaceRules::room_before`]), so it
//! stands on that text's side of the right-to-left run.
//!
//! The measure scan finds seams with [`Seams`]; the breaker asks about one
//! line edge with [`AutospaceRules::gap_after`], and about each cluster of a
//! reshaped piece with [`AutospaceRules::gap_after_near`]. Each sizes the
//! room with [`AutospaceRules::gap`]. A seam inside one cluster is not looked
//! for.

use crate::data::Id;
use crate::stages::analysis::{BidiLevel, ClusterId, Paragraph, ParagraphFlags};
use crate::stages::content::{
    Content, ContentFlags, Facts, Item, ItemFlags, ItemId, ItemKind, TextFactsId, TextFlags,
};
use crate::stages::fonts::Fonts;
use crate::stages::shape::ShapedRunId;
use crate::stages::{LineStages, Slot};
use crate::style::{FirstLineVariant, UnicodeBidi};
use crate::unicode::{self, EastAsianSpacing};
use crate::unit::TextUnit;
use crate::work;

/// What a character is to a seam, with conditional punctuation resolved.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    /// Neither side of any seam.
    Other,
    /// A narrow letter, or conditional punctuation in Chinese.
    Letter,
    /// A narrow decimal digit.
    Digit,
    /// An ideograph, kana, Hangul and the like.
    Ideograph,
}

impl Kind {
    /// What `ch` is to a seam, `chinese` where the block's language is
    /// Chinese, or `None` for a mark, which is stepped over.
    fn from_char(ch: char, chinese: bool) -> Option<Self> {
        // Below U+00AA nothing is wide and nothing a mark: letters, digits
        // and conditional punctuation are the ASCII ones.
        if ch.is_ascii() {
            return Some(if ch.is_ascii_alphabetic() {
                Self::Letter
            } else if ch.is_ascii_digit() {
                Self::Digit
            } else if chinese
                && unicode::rare_props(ch).east_asian_spacing() == EastAsianSpacing::Conditional
            {
                Self::Letter
            } else {
                Self::Other
            });
        }
        Self::from_spacing(unicode::rare_props(ch).east_asian_spacing(), chinese)
    }

    /// What a character of East_Asian_Spacing `spacing` is to a seam,
    /// conditional punctuation a letter where `chinese` and neither side
    /// elsewhere, or `None` for a mark.
    fn from_spacing(spacing: EastAsianSpacing, chinese: bool) -> Option<Self> {
        Some(match spacing {
            EastAsianSpacing::Mark => return None,
            EastAsianSpacing::Wide => Self::Ideograph,
            EastAsianSpacing::Narrow => Self::Letter,
            EastAsianSpacing::NarrowDigit => Self::Digit,
            EastAsianSpacing::Conditional if chinese => Self::Letter,
            EastAsianSpacing::Conditional | EastAsianSpacing::Other => Self::Other,
        })
    }

    /// Whether it is either side of some seam: an ideograph, or a narrow
    /// letter or digit.
    fn takes_seams(self) -> bool {
        self != Self::Other
    }

    /// Returns the narrow side's kind if a seam falls between it and `after`.
    ///
    /// A seam is an ideograph beside a narrow letter or digit, either way
    /// round. The narrow side's kind says which style value takes the seam.
    fn seam(self, after: Self) -> Option<Self> {
        match (self, after) {
            (Self::Ideograph, narrow @ (Self::Letter | Self::Digit))
            | (narrow @ (Self::Letter | Self::Digit), Self::Ideograph) => Some(narrow),
            _ => None,
        }
    }
}

/// One side of a seam: the kind of the character nearest it, and what holds
/// that character.
#[derive(Copy, Clone, Debug)]
pub(super) struct SeamSide {
    kind: Kind,
    /// The item holding it: a seam inside one item is written into it.
    item: ItemId,
    /// That item's text facts, in the line's variant.
    facts: TextFactsId,
    /// Its level is odd: the item reads right to left.
    rtl: bool,
}

/// Item facts and direction held for one measurement segment.
#[derive(Copy, Clone, Debug)]
pub(super) struct SeamContext {
    item: ItemId,
    facts: TextFactsId,
    rtl: bool,
}

impl SeamContext {
    pub(super) fn new(
        id: ItemId,
        item: &Item,
        facts: TextFactsId,
        level: BidiLevel,
    ) -> Option<Self> {
        (!item.flags.contains(ItemFlags::ANNOTATION)).then_some(Self {
            item: id,
            facts,
            rtl: level.is_rtl(),
        })
    }

    fn side(self, kind: Option<Kind>) -> Option<SeamSide> {
        Some(SeamSide {
            kind: kind?,
            item: self.item,
            facts: self.facts,
            rtl: self.rtl,
        })
    }
}

/// Both sides of a cluster, classified once and reused at the next boundary.
#[derive(Copy, Clone, Debug)]
struct ClusterSides {
    at: ClusterId,
    first: Option<Kind>,
    last: Option<Kind>,
}

impl ClusterSides {
    fn new(at: ClusterId, text: &str, chinese: bool) -> Self {
        // Stop at each nearest non-mark character. The remaining iterator
        // excludes the first, so a single-character cluster is classified once.
        let mut chars = text.chars();
        let first = chars.find_map(|ch| Kind::from_char(ch, chinese));
        let last = chars
            .rev()
            .find_map(|ch| Kind::from_char(ch, chinese))
            .or(first);
        Self { at, first, last }
    }
}

impl SeamSide {
    /// Returns the side a cluster with characters `text` offers a seam.
    ///
    /// The side reads the first character where `first`, else the last.
    /// Returns `None` where the cluster is stepped over: every character is a
    /// mark, or the cluster is ruby annotation text.
    fn new(
        rules: AutospaceRules,
        text: &str,
        (id, item): (ItemId, &Item),
        facts: TextFactsId,
        level: BidiLevel,
        first: bool,
    ) -> Option<Self> {
        if item.flags.contains(ItemFlags::ANNOTATION) {
            return None;
        }
        let kind = if first {
            first_kind(text, rules.chinese)
        } else {
            last_kind(text, rules.chinese)
        };
        Some(Self {
            kind: kind?,
            item: id,
            facts,
            rtl: level.is_rtl(),
        })
    }
}

/// What every seam of a block reads: whether any style sets the property at
/// all, and the block's language.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AutospaceRules {
    /// Some style of the content takes a seam: without one no cluster is
    /// looked at.
    any: bool,
    /// The block's language is Chinese, under which conditional punctuation
    /// is narrow.
    chinese: bool,
}

impl AutospaceRules {
    /// The rules of `content`'s block.
    pub(crate) fn new(content: &Content) -> Self {
        let any = content.flags.contains(ContentFlags::AUTOSPACE);
        // The builder resolves the block's language (`BlockFacts`).
        let chinese = any && content.block.chinese;
        Self { any, chinese }
    }

    /// Whether some style takes a seam.
    pub(super) fn any(self) -> bool {
        self.any
    }

    /// The room at a seam from `left` to `right`, their text facts among
    /// `facts`, set in `fonts`, or nothing where the two make none (see the
    /// module documentation).
    fn gap(facts: &Facts, fonts: &Fonts, left: SeamSide, right: SeamSide) -> TextUnit {
        let none = TextUnit::from_raw(0);
        let Some(narrow) = left.kind.seam(right.kind) else {
            return none;
        };
        if !takes(facts, left.facts, narrow) || !takes(facts, right.facts, narrow) {
            return none;
        }
        // Written into the earlier item where it reads left to right, else
        // the later, and inside one item into it: its font sizes the room.
        let owner = if left.item == right.item || !left.rtl {
            left.facts
        } else {
            right.facts
        };
        fonts.autospace_gap(facts.text_request(owner))
    }

    /// Returns the room `text-autospace` puts after `cluster` of `paragraph`.
    ///
    /// This is what the measure scan added to the prefix there, sized by the
    /// same [`gap`](Self::gap). The breaker takes it off a line that ends at
    /// the seam. Returns zero past a paragraph's end or where no style sets
    /// the property.
    ///
    /// Where the characters either side leave a seam possible, it seeks the
    /// first item after `cluster`, and the shaping runs either side only
    /// where the paragraph's levels differ.
    pub(crate) fn gap_after(
        self,
        stages: &LineStages<'_>,
        paragraph: &Paragraph,
        cluster: ClusterId,
    ) -> TextUnit {
        let next = ClusterId::new(cluster.get() + 1);
        let items = &stages.analysis.item_clusters;
        self.seam_after(stages, paragraph, cluster, || {
            (items.cursor_containing(next).id(), None)
        })
        .map_or(TextUnit::from_raw(0), |(_, _, room)| room)
    }

    /// Returns the room [`gap_after`](Self::gap_after) does, for a walk
    /// forward over the clusters of `paragraph`.
    ///
    /// `near` is where the walk stands, at or before the cluster after
    /// `cluster`, or `None` before it has stood anywhere. It moves to the
    /// cluster after `cluster` only where the characters leave a seam
    /// possible, stepping from where it stood, or seeking the first time.
    /// The breaker adds the room to each cluster of a line edge it reshapes.
    pub(crate) fn gap_after_near(
        self,
        stages: &LineStages<'_>,
        paragraph: &Paragraph,
        cluster: ClusterId,
        near: &mut Option<Slot>,
    ) -> TextUnit {
        let next = ClusterId::new(cluster.get() + 1);
        let slot = || {
            let slot = match *near {
                Some(slot) => slot.advance(stages, next),
                None => Slot::new(stages, next),
            };
            *near = Some(slot);
            (slot.item(), Some(slot.run()))
        };
        self.seam_after(stages, paragraph, cluster, slot)
            .map_or(TextUnit::from_raw(0), |(_, _, room)| room)
    }

    /// Returns the room of the seam at boundary `at` that line layout draws
    /// before the text after it rather than after the text before it.
    ///
    /// That is the room where the side before reads right to left and the
    /// side after left to right. Chrome's `TextAutoSpace` writes it into the
    /// later item, as an offset before its first glyph. The prefix holds it
    /// before `at`, as for every seam.
    ///
    /// `at` is a boundary inside `paragraph`, past its first cluster. It is
    /// zero unless some style sets the property and the paragraph has text
    /// at another level. Otherwise it asks as [`gap_after`](Self::gap_after)
    /// does.
    ///
    /// The tests of the property and the text's levels are inlined, so a
    /// reader of text that sets neither makes no call.
    #[inline]
    pub(crate) fn room_before(
        self,
        stages: &LineStages<'_>,
        paragraph: &Paragraph,
        at: ClusterId,
    ) -> TextUnit {
        if !self.may_have_room_before(stages) {
            return TextUnit::from_raw(0);
        }
        self.mixed_room_before(stages, paragraph, at)
    }

    /// Returns whether [`room_before`](Self::room_before) may find room in
    /// `stages`: some style sets the property and some paragraph reads right
    /// to left.
    #[inline]
    pub(crate) fn may_have_room_before(self, stages: &LineStages<'_>) -> bool {
        self.any
            && stages
                .analysis
                .flags
                .contains(ParagraphFlags::RIGHT_TO_LEFT)
    }

    /// Returns [`room_before`](Self::room_before)'s room where some style
    /// sets the property and some paragraph reads right to left.
    #[inline(never)]
    fn mixed_room_before(
        self,
        stages: &LineStages<'_>,
        paragraph: &Paragraph,
        at: ClusterId,
    ) -> TextUnit {
        let none = TextUnit::from_raw(0);
        let Some(cluster) = at.get().checked_sub(1).map(ClusterId::new) else {
            return none;
        };
        if !paragraph.flags.contains(ParagraphFlags::MIXED_LEVELS) {
            return none;
        }
        let slot = || {
            let slot = Slot::new(stages, at);
            (slot.item(), Some(slot.run()))
        };
        match self.seam_after(stages, paragraph, cluster, slot) {
            Some((left, right, room)) if left.rtl && !right.rtl => room,
            _ => none,
        }
    }

    /// Returns both sides of the seam after `cluster` of `paragraph` and
    /// its room, or `None` where there is no seam.
    ///
    /// The characters either side decide nearly every boundary. Where they
    /// do not, it asks `next` for the first item after `cluster`, and the
    /// shaping run of the cluster after it where the caller holds it. It
    /// walks back from that item, item by item, for the side before.
    ///
    /// A side's level comes from its shaping run's script run, read only
    /// where the paragraph's levels differ. The run after the seam is the
    /// one `next` gives, or else found by its rank. The walk back steps back
    /// the runs from it.
    fn seam_after(
        self,
        stages: &LineStages<'_>,
        paragraph: &Paragraph,
        cluster: ClusterId,
        next: impl FnOnce() -> (ItemId, Option<ShapedRunId>),
    ) -> Option<(SeamSide, SeamSide, TextUnit)> {
        let (content, analysis) = (stages.content, stages.analysis);
        let none = None;
        let count = analysis.clusters.len();
        if !self.any || cluster.get() + 1 >= count {
            return none;
        }
        let after = ClusterId::new(cluster.get() + 1);
        // The characters alone first, which nearly every boundary is decided
        // by: most of a CJK line's ends fall between two ideographs, and no
        // item need be looked for there. Where no annotation text can stand
        // between, the cluster before is the seam's other side where it has
        // a character that is not a mark. Each side reads the one character
        // nearest the seam.
        let source = stages.text();
        let text_of = |at: ClusterId| analysis.clusters.text(source, at);
        let Some(right_kind) = first_kind(text_of(after), self.chinese) else {
            return none;
        };
        if !right_kind.takes_seams() {
            return none;
        }
        if !content.flags.contains(ContentFlags::RUBY)
            && let Some(left_kind) = last_kind(text_of(cluster), self.chinese)
            && left_kind.seam(right_kind).is_none()
        {
            return none;
        }
        let items = &analysis.item_clusters;
        let runs = &stages.shaped.runs;
        let mixed = paragraph.flags.contains(ParagraphFlags::MIXED_LEVELS);
        // The run shaping `at`, stepped back from `near`, or else ranked:
        // read only where the levels differ.
        let run_back = |at: ClusterId, near: Option<ShapedRunId>| {
            if !mixed {
                return None;
            }
            let Some(mut run) = near else {
                return runs.containing(at);
            };
            while runs.run_clusters(run).start > at && run.get() > 0 {
                run = ShapedRunId::new(run.get() - 1);
            }
            Some(run)
        };
        let side = |at: ClusterId, run: Option<ShapedRunId>, id: ItemId, item: &Item, first| {
            let level = run
                .and_then(|run| runs.get(run))
                .and_then(|run| analysis.runs.get(run.script_run))
                .map_or(paragraph.level, |run| run.level);
            let facts = stages.text_facts(item.node);
            SeamSide::new(self, text_of(at), (id, item), facts, level, first)
        };
        let (mut id, near) = next();
        let mut run = run_back(after, near);
        // The side after: the next cluster's first character, where no item
        // at the boundary between ends the seam.
        let right = loop {
            work::step();
            let Some(item) = content.items.get(id) else {
                return none;
            };
            if !items.range(id).is_empty() {
                break side(after, run, id, item, true);
            }
            if ends_seams(content, item) {
                return none;
            }
            id = ItemId::new(id.get() + 1);
        };
        let Some(right) = right else {
            return none;
        };
        // The side before: the last cluster not stepped over, back to the
        // paragraph's start, where no item passed between ends the seam.
        let mut at = cluster;
        loop {
            work::step();
            // Back to the item holding it, past those at the boundary after.
            while items.start(id) > at {
                let Some(before) = id.get().checked_sub(1) else {
                    return none;
                };
                id = ItemId::new(before);
                let Some(item) = content.items.get(id) else {
                    return none;
                };
                if items.range(id).is_empty() && ends_seams(content, item) {
                    return none;
                }
            }
            let Some(item) = content.items.get(id) else {
                return none;
            };
            run = run_back(at, run);
            if let Some(left) = side(at, run, id, item, false) {
                let room = Self::gap(&content.facts, stages.fonts, left, right);
                return (room.raw() != 0).then_some((left, right, room));
            }
            if at <= paragraph.start {
                return none;
            }
            at = ClusterId::new(at.get() - 1);
        }
    }
}

/// The kind of the first character of `text` that is not a mark, or `None`
/// where every one is: the side a cluster offers a seam before it.
fn first_kind(text: &str, chinese: bool) -> Option<Kind> {
    text.chars().find_map(|ch| Kind::from_char(ch, chinese))
}

/// The kind of the last character of `text` that is not a mark, or `None`
/// where every one is: the side a cluster offers a seam after it. Each side
/// is asked of the one end it offers, so a cluster of one character, as
/// nearly every CJK one is, is classified once and not twice.
fn last_kind(text: &str, chinese: bool) -> Option<Kind> {
    text.chars()
        .rev()
        .find_map(|ch| Kind::from_char(ch, chinese))
}

/// Returns whether text with the facts `text` takes the seam whose narrow
/// side is `narrow`.
///
/// A letter takes it under `ideograph-alpha`, and a digit under
/// `ideograph-numeric`. Neither does where the text is set upright in a
/// vertical line, as Chrome ends a seam at an upright item
/// (`kVerticalUpright`). The builder folds uprightness into both flags.
fn takes(facts: &Facts, text: TextFactsId, narrow: Kind) -> bool {
    let text = facts.text(text);
    match narrow {
        Kind::Letter => text.has(TextFlags::AUTOSPACE_ALPHA),
        Kind::Digit => text.has(TextFlags::AUTOSPACE_NUMERIC),
        Kind::Other | Kind::Ideograph => false,
    }
}

/// Returns whether `item`, at a boundary with no text of its own, ends any
/// seam across it.
///
/// A float or an absolutely positioned box's anchor ends one, since Chrome
/// writes each as a U+FFFC. So does a box whose `unicode-bidi` opens and
/// closes bidi controls, which Chrome writes into the text. The box facts
/// decide, and `::first-line` doesn't change them.
pub(super) fn ends_seams(content: &Content, item: &Item) -> bool {
    match item.kind {
        ItemKind::Float | ItemKind::Absolute => true,
        ItemKind::Open | ItemKind::Close => {
            let id = content
                .nodes
                .box_facts(item.node, FirstLineVariant::Standard);
            content.facts.box_facts(id).bidi != UnicodeBidi::Normal
        }
        _ => false,
    }
}

/// The seams of one pass over the clusters, in order: the side before the
/// next seam, carried over clusters that are stepped over.
#[derive(Copy, Clone, Debug)]
pub(super) struct Seams {
    rules: AutospaceRules,
    /// The side the last cluster that is not stepped over offers after it,
    /// or `None` where something since ends any seam.
    last: Option<SeamSide>,
    classified: Option<ClusterSides>,
}

impl Seams {
    /// No seam yet, under `rules`.
    pub(super) fn new(rules: AutospaceRules) -> Self {
        Self {
            rules,
            last: None,
            classified: None,
        }
    }

    /// Classifies a cluster only on its first visit. Look-ahead copies this
    /// cache with the seam state, so its reads do not change the main scan.
    pub(super) fn side(
        &mut self,
        context: Option<SeamContext>,
        at: ClusterId,
        text: &str,
        first: bool,
    ) -> Option<SeamSide> {
        let context = context?;
        let sides = match self.classified {
            Some(sides) if sides.at == at => sides,
            _ => {
                let sides = ClusterSides::new(at, text, self.rules.chinese);
                self.classified = Some(sides);
                sides
            }
        };
        context.side(if first { sides.first } else { sides.last })
    }

    /// Returns the room after a cluster and moves the scan past it.
    ///
    /// `here` is the cluster's side after it, `None` where it is stepped
    /// over. `next` is the following cluster's side before it. `ended` says
    /// something between them ends any seam.
    pub(super) fn after(
        &mut self,
        content: &Content,
        fonts: &Fonts,
        here: Option<SeamSide>,
        next: Option<SeamSide>,
        ended: bool,
    ) -> TextUnit {
        let left = here.or(self.last);
        let room = match (left, next) {
            (Some(left), Some(right)) if !ended => {
                AutospaceRules::gap(&content.facts, fonts, left, right)
            }
            _ => TextUnit::from_raw(0),
        };
        self.last = if ended { None } else { left };
        room
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_cluster_sides_match_independent_end_reads() {
        let at = ClusterId::new(0);
        for chinese in [false, true] {
            for text in [
                "",
                "水",
                "A",
                "1",
                "\u{301}",
                "\u{301}水\u{302}",
                "A\u{301}水",
                "水A",
                "👩‍👩‍👧‍👦",
                "·",
            ] {
                let sides = ClusterSides::new(at, text, chinese);
                assert_eq!(sides.first, first_kind(text, chinese), "{text:?}");
                assert_eq!(sides.last, last_kind(text, chinese), "{text:?}");
            }
        }
    }
}
