//! How far a ruby column overhangs the text beside it, by the config's
//! rule: Chrome's, or the Japanese Layout Requirements'.

use super::ruby::{Sizing, keep_tab_reach};
use crate::config::RubyOverhangRule;
use crate::data::Id;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{ItemId, NodeId, TextFactsId, TextFlags};
use crate::stages::measure::tabs::tab_advance;
use crate::stages::measure::{MeasuredText, RubySide, RubySpread, TabReach};
use crate::style::{RubyAlign, RubyGroup, RubyOverhang};
use crate::unicode::TextSpacingClass;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::{unicode, work};

impl Sizing<'_, '_> {
    /// Returns how far `column` overhangs the text before and after it, by
    /// `Config::ruby_overhang`'s rule.
    ///
    /// `widest` is its widest annotation's width and font size. Zero where the
    /// text beside can't be reached over. Keeps in `out` how far each tab after
    /// the column moves.
    pub(super) fn overhang(
        &self,
        column: Overhanging,
        widest: Option<(LayoutUnit, f32)>,
        out: &mut MeasuredText,
    ) -> (LayoutUnit, LayoutUnit) {
        let none = (LayoutUnit::ZERO, LayoutUnit::ZERO);
        let Some((_, size)) = widest else {
            return none;
        };
        if column.space <= LayoutUnit::ZERO || column.ruby.group.overhang == RubyOverhang::None {
            return none;
        }
        match self.scan.input.ruby_overhang {
            RubyOverhangRule::AdjacentText => self.chrome_overhang(column, size, out),
            RubyOverhangRule::KanaOnly => self.jlreq_overhang(column, size, out),
        }
    }

    /// Returns Chrome's overhang (Blink's `GetOverhang`,
    /// `CanApplyStartOverhang` and `CommitPendingEndOverhang`).
    ///
    /// - `auto`: the lesser of the base's inset and half the annotation's font
    ///   size, at each side, or the end only under `ruby-align: start`. At
    ///   most half the text there, and never over a tab.
    /// - `spaces`: as far as the blank beside reaches, within the inset
    ///   ([`blank_before`](Self::blank_before), [`end_take`](Self::end_take)).
    ///   The start stops where the previous column's end overhang reaches.
    /// - `space-between`: none.
    ///
    /// It reaches only over text no larger than the ruby's and not marked for
    /// emphasis on the annotation's side.
    fn chrome_overhang(
        &self,
        column: Overhanging,
        size: f32,
        out: &mut MeasuredText,
    ) -> (LayoutUnit, LayoutUnit) {
        let Overhanging {
            space,
            opportunities,
            ruby,
            before,
            after,
            ..
        } = column;
        let align = ruby.group.align;
        if align == RubyAlign::SpaceBetween {
            return (LayoutUnit::ZERO, LayoutUnit::ZERO);
        }
        // Half the annotation's font size in whole pixels, as Blink's
        // `FontSize() / 2` with `FontSize()` rounded (`ComputedPixelSize`): a 17px
        // annotation overhangs by 8, not 8.5 (measured with Chrome 153).
        let half = LayoutUnit::from_px_whole(size).half().floor_px();
        // The base's inset each side under `space-around` (`ComputeRubyBaseInset`).
        let inset = RubySpread::from_base(RubyAlign::SpaceAround, space, opportunities).start;
        let spaces = ruby.group.overhang == RubyOverhang::Spaces;
        let (start, end) = match (spaces, align) {
            (false, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(half)),
            (false, _) => (inset.min(half), inset.min(half)),
            (true, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(inset + inset)),
            (true, _) => (inset, inset),
        };
        let scan = self.scan;
        let side = RubySide::from_position(ruby.group.position, 0);
        let may = |beside: &TextBeside| {
            let text = self.item_text(beside.item);
            let facts = scan.input.content.facts.text(text);
            let marked =
                facts.has(TextFlags::EMPHASIS) && RubySide::from(facts.emphasis_side) == side;
            scan.input.size(text) <= ruby.size && !marked
        };
        let is_tab = |beside: &TextBeside| scan.input.analysis.clusters.is_tab(beside.cluster);
        // Under `spaces` the blank bounds it; otherwise half the text, unless a tab
        // stands beside the column.
        let mut start = match before {
            Some(beside) if may(&beside) && spaces => start.min(self.blank_before(&beside)),
            Some(beside) if may(&beside) && !is_tab(&beside) => {
                start.min(beside.width.to_layout().half())
            }
            _ => LayoutUnit::ZERO,
        };
        if spaces && let Some(limit) = self.start_limit {
            start = start.min(limit);
        }
        let end = match after {
            Some(beside) if may(&beside) && spaces => {
                self.end_overhang(&column, &beside, start, end, out)
            }
            Some(beside) if may(&beside) && !is_tab(&beside) => {
                end.min(beside.width.to_layout().half())
            }
            _ => LayoutUnit::ZERO,
        };
        (start, end)
    }

    /// Returns the blank the text before a column, `beside`, offers it, as
    /// Chrome's `spaces` reads it (`GetOverhang`).
    ///
    /// Counting back from the column: spaces and tabs, then the blank half of a
    /// full-width closing mark or a quarter of a middle dot. The tab run counts
    /// once, as wide as on the paragraph's line.
    fn blank_before(&self, beside: &TextBeside) -> LayoutUnit {
        let prefix = self.prefix;
        let cluster = beside.cluster;
        let clusters = &self.scan.input.analysis.clusters;
        // A passed cluster's advance: the difference of two sums.
        let advance = |at: ClusterId| -> InlineLayoutUnit {
            let from = prefix.get(at).unwrap_or_default();
            let to = if at == cluster {
                beside.end
            } else {
                prefix.get(ClusterId::new(at.get() + 1)).unwrap_or(from)
            };
            to - from
        };
        let mut blank = InlineLayoutUnit::ZERO;
        let mut tabs_taken = false;
        let mut at = cluster;
        let mark = loop {
            work::step();
            let class = clusters.attrs(at).map(ClusterAttrs::class);
            match class {
                Some(ClusterClass::Tab) => match self.tabs_before {
                    Some(run) if run.last == at && !tabs_taken => {
                        blank += InlineLayoutUnit::from_layout(run.width);
                        tabs_taken = true;
                    }
                    _ if tabs_taken => {}
                    _ => break None,
                },
                Some(
                    ClusterClass::Space | ClusterClass::OtherSpace | ClusterClass::NoBreakSpace,
                ) => {
                    blank += advance(at);
                }
                Some(_) => break Some(at),
                None => break None,
            }
            let Some(before) = at.get().checked_sub(1) else {
                break None;
            };
            at = ClusterId::new(before);
        };
        let facing = mark.map_or(LayoutUnit::ZERO, |mark| {
            self.facing(beside.item, mark, false, advance(mark))
        });
        (blank + InlineLayoutUnit::from_layout(facing)).to_layout()
    }

    /// Returns the blank `mark`, advancing `advance`, offers a column it faces:
    /// half a full-width opening mark after it (`forward`) or closing mark
    /// before it, or a quarter of a middle dot.
    ///
    /// A mark narrower than full width offers none (`IsFullWidthGlyph`). A
    /// middle dot counts at any width, as Chrome's `MaybeHanKerningMiddle`
    /// reads only the character.
    fn facing(
        &self,
        item: ItemId,
        mark: ClusterId,
        forward: bool,
        advance: InlineLayoutUnit,
    ) -> LayoutUnit {
        let content = self.scan.input.content;
        let clusters = &self.scan.input.analysis.clusters;
        let Some(ch) = clusters.first_char(content.text(self.scan.variant), mark) else {
            return LayoutUnit::ZERO;
        };
        let size = self.size(item, mark);
        let full = || advance.to_layout() + LayoutUnit::EPSILON >= size - size.divided(10);
        match unicode::rare_props(ch).text_spacing() {
            TextSpacingClass::Open if forward && full() => size.half(),
            TextSpacingClass::Close if !forward && full() => size.half(),
            TextSpacingClass::Middle => size.half().half(),
            _ => LayoutUnit::ZERO,
        }
    }

    /// Returns how far `column`, with start overhang `start`, overhangs the
    /// blank after it, `beside`, up to `pending`. Keeps in `out` how far each
    /// tab there moves.
    ///
    /// Chrome's `CommitPendingEndOverhang` takes the overhang off the items
    /// after the column one by one ([`end_take`](Self::end_take)); the prefix
    /// takes it off the column. Each tab is sized where Chrome sizes it.
    fn end_overhang(
        &self,
        column: &Overhanging,
        beside: &TextBeside,
        start: LayoutUnit,
        pending: LayoutUnit,
        out: &mut MeasuredText,
    ) -> LayoutUnit {
        let from = column.line_start - InlineLayoutUnit::from_layout(start)
            + InlineLayoutUnit::from_layout(column.width);
        let taken = self.end_take(beside, from, pending, |_, _| {});
        if self.scan.has_tabs {
            self.end_take(beside, from, pending, |tab, before| {
                let reach = TabReach {
                    tab,
                    shift: taken - before,
                };
                keep_tab_reach(reach, out);
            });
        }
        taken
    }

    /// Returns how much of `pending` the blank after a column, `beside`, takes,
    /// the column ending at `from` on its paragraph's line, as Chrome's
    /// `CommitPendingEndOverhang` does.
    ///
    /// Each item takes what it can of what is left: a tab run its width, sized
    /// from where it starts; a space run its advance, plus the blank of a mark
    /// facing the column. Calls `tab` with each tab and what was taken before it.
    fn end_take(
        &self,
        beside: &TextBeside,
        from: InlineLayoutUnit,
        pending: LayoutUnit,
        mut tab: impl FnMut(ClusterId, LayoutUnit),
    ) -> LayoutUnit {
        let prefix = self.prefix;
        let clusters = &self.scan.input.analysis.clusters;
        let mut left = InlineLayoutUnit::from_layout(pending);
        let mut taken = InlineLayoutUnit::ZERO;
        // Where Chrome's breaker stands, and the run being added up.
        let mut pen = from;
        let (mut spaces, mut tabs) = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        // Takes what an item `width` wide can of what is left.
        fn take(
            width: &mut InlineLayoutUnit,
            left: &mut InlineLayoutUnit,
            pen: &mut InlineLayoutUnit,
        ) -> InlineLayoutUnit {
            let took = (*width).min(*left);
            *left = *left - took;
            *pen += *width - took;
            *width = InlineLayoutUnit::ZERO;
            took
        }
        let mut at = beside.cluster;
        loop {
            work::step();
            if left <= InlineLayoutUnit::ZERO {
                break;
            }
            let class = clusters.attrs(at).map(ClusterAttrs::class);
            match class {
                Some(ClusterClass::Tab) => {
                    if spaces > InlineLayoutUnit::ZERO {
                        taken += take(&mut spaces, &mut left, &mut pen);
                    }
                    tab(at, taken.to_layout());
                    let stops = self.scan.tab_stops(self.text(beside.item, at));
                    let width = stops.map_or(LayoutUnit::ZERO, |stops| {
                        tab_advance(LayoutUnit::ZERO, pen + tabs, stops)
                    });
                    tabs += InlineLayoutUnit::from_layout(width);
                }
                Some(
                    ClusterClass::Space | ClusterClass::OtherSpace | ClusterClass::NoBreakSpace,
                ) => {
                    if tabs > InlineLayoutUnit::ZERO {
                        taken += take(&mut tabs, &mut left, &mut pen);
                    }
                    spaces += prefix.get(at).unwrap_or_default();
                }
                Some(_) => {
                    if tabs > InlineLayoutUnit::ZERO {
                        taken += take(&mut tabs, &mut left, &mut pen);
                    }
                    let advance = prefix.get(at).unwrap_or_default();
                    let facing = self.facing(beside.item, at, true, advance);
                    spaces += InlineLayoutUnit::from_layout(facing);
                    break;
                }
                None => break,
            }
            at = ClusterId::new(at.get() + 1);
        }
        taken += take(&mut tabs, &mut left, &mut pen);
        taken += take(&mut spaces, &mut left, &mut pen);
        taken.to_layout()
    }

    /// Returns the computed font size of the text holding `cluster`, truncated
    /// to the grid as Chrome's `LayoutUnit(font_size)` is. Searches from `item`;
    /// the block's size where no item holds it.
    fn size(&self, item: ItemId, cluster: ClusterId) -> LayoutUnit {
        let text = self.text(item, cluster);
        LayoutUnit::from_px_truncated(self.scan.input.size(text))
    }

    /// Returns the text facts of the item holding `cluster`, searching from
    /// `item`.
    fn text(&self, item: ItemId, cluster: ClusterId) -> TextFactsId {
        let content = self.scan.input.content;
        let items = &self.scan.input.analysis.item_clusters;
        // The first item at the cluster's start may be a box's edge; the cluster's
        // own is the first after it that holds clusters.
        let id = items.walk_to_cluster(item, cluster);
        if items.range(id).contains(&cluster) {
            self.item_text(id)
        } else {
            content.nodes.text_facts(NodeId::BLOCK, self.scan.variant)
        }
    }

    /// Returns the text facts of item `id`'s node in the variant, or the
    /// block's past the last item.
    fn item_text(&self, id: ItemId) -> TextFactsId {
        let content = self.scan.input.content;
        let node = content
            .items
            .get(id)
            .map_or(NodeId::BLOCK, |item| item.node);
        content.nodes.text_facts(node, self.scan.variant)
    }

    /// Returns the overhang of the Japanese Layout Requirements (JLREQ 3.3.7).
    ///
    /// - `auto`: over kana only, by at most one annotation em and half the
    ///   difference.
    /// - `spaces`: into blank only, by at most half the difference.
    ///
    /// It never reaches over another column.
    fn jlreq_overhang(
        &self,
        column: Overhanging,
        size: f32,
        out: &mut MeasuredText,
    ) -> (LayoutUnit, LayoutUnit) {
        let Overhanging {
            space,
            ruby,
            before,
            after,
            ..
        } = column;
        let ruby = ruby.group;
        let content = self.scan.input.content;
        let clusters = &self.scan.input.analysis.clusters;
        let half = space.half();
        let em = LayoutUnit::from_px(size);
        let spaces = ruby.overhang == RubyOverhang::Spaces;
        if spaces {
            let start = before.map_or(LayoutUnit::ZERO, |beside| {
                self.blank_before(&beside).min(half)
            });
            let end = after.map_or(LayoutUnit::ZERO, |beside| {
                self.end_overhang(&column, &beside, start, half, out)
            });
            return (start, end);
        }
        let reach = |beside: Option<TextBeside>| {
            let Some(beside) = beside else {
                return LayoutUnit::ZERO;
            };
            let source = content.text(self.scan.variant);
            let kana = clusters
                .first_char(source, beside.cluster)
                .is_some_and(|ch| {
                    matches!(
                        u32::from(ch),
                        0x3041..=0x309F | 0x30A0..=0x30FA | 0x30FC..=0x30FF | 0x31F0..=0x31FF
                    )
                });
            if kana { em.min(half) } else { LayoutUnit::ZERO }
        };
        (reach(before), reach(after))
    }
}

/// A ruby column as its overhang reads it.
#[derive(Copy, Clone, Debug)]
pub(super) struct Overhanging {
    /// How much wider the column is than its base.
    pub(super) space: LayoutUnit,
    /// How many justification opportunities its base has.
    pub(super) opportunities: u32,
    pub(super) ruby: Ruby,
    /// The text before and after the column, where it can be reached.
    pub(super) before: Option<TextBeside>,
    pub(super) after: Option<TextBeside>,
    /// Where the column starts on its paragraph's line, before overhang, and
    /// its width.
    pub(super) line_start: InlineLayoutUnit,
    pub(super) width: LayoutUnit,
}

/// A ruby container's ruby properties and computed font size.
#[derive(Copy, Clone, Debug)]
pub(super) struct Ruby {
    pub(super) group: RubyGroup,
    pub(super) size: f32,
}

/// The text beside a ruby column.
#[derive(Copy, Clone, Debug)]
pub(super) struct TextBeside {
    pub(super) item: ItemId,
    /// Its width: up to the column before it, its whole width after it.
    pub(super) width: InlineLayoutUnit,
    /// The cluster next to the column.
    pub(super) cluster: ClusterId,
    /// Before the column, the running sum where the text ends, not yet in the
    /// prefix. Zero after it.
    pub(super) end: InlineLayoutUnit,
}
