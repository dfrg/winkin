//! How far a ruby column overhangs the text beside it, by the config's
//! rule: Chrome's, or the Japanese Layout Requirements'.

use super::ruby::Sizing;
use crate::config::RubyOverhangRule;
use crate::data::Id;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{ItemId, NodeId, TextFactsId, TextFlags};
use crate::stages::measure::{RubySide, RubySpread, TabReach};
use crate::style::{RubyAlign, RubyGroup, RubyOverhang};
use crate::unicode::TextSpacingClass;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::{unicode, work};

impl Sizing<'_, '_> {
    /// Returns how far `column` overhangs the text before it and after it,
    /// by `Config::ruby_overhang`'s rule.
    ///
    /// `widest` is its widest annotation's width and font size. The overhang
    /// is zero where the text beside it can't be reached over.
    ///
    /// Also returns how far the overhang reaches into a tab beside the
    /// column, before it and after it, where it does.
    pub(super) fn overhang(
        &self,
        column: Overhanging,
        widest: Option<(LayoutUnit, f32)>,
    ) -> ((LayoutUnit, LayoutUnit), [Option<TabReach>; 2]) {
        let none = ((LayoutUnit::ZERO, LayoutUnit::ZERO), [None, None]);
        let Some((_, size)) = widest else {
            return none;
        };
        if column.space <= LayoutUnit::ZERO || column.ruby.group.overhang == RubyOverhang::None {
            return none;
        }
        let (start, end, tabs) = match self.scan.input.ruby_overhang {
            RubyOverhangRule::AdjacentText => self.chrome_overhang(column, size),
            RubyOverhangRule::KanaOnly => self.jlreq_overhang(column, size),
        };
        // The overhang reaches into a tab past the blank before it.
        let reach = |tab: Option<(ClusterId, LayoutUnit)>, overhang: LayoutUnit, after: bool| {
            tab.filter(|&(_, blank)| overhang > blank)
                .map(|(tab, blank)| TabReach {
                    tab,
                    reach: overhang - blank,
                    after,
                })
        };
        let reaches = [reach(tabs.0, start, false), reach(tabs.1, end, true)];
        ((start, end), reaches)
    }

    /// Returns Chrome's overhang (Blink's `GetOverhang`,
    /// `CanApplyStartOverhang` and `CommitPendingEndOverhang`).
    ///
    /// - Under `auto`: the base's inset or half the annotation's font size,
    ///   whichever is less, at each side. Under `ruby-align: start`, at the
    ///   end only. Never further than half the text there.
    ///   It never reaches a tab.
    /// - Under `spaces`: as far as the blank beside the column reaches,
    ///   within the inset. A tab after the column is blank. The start stops
    ///   where the last column's end overhang reaches.
    /// - Under `space-between`: none.
    ///
    /// It reaches only over text no larger than the ruby's, and not marked
    /// for emphasis on the annotation's side.
    ///
    /// Also returns the tabs the blank reaches before and after the column,
    /// each with the blank nearer the column.
    fn chrome_overhang(
        &self,
        column: Overhanging,
        size: f32,
    ) -> (LayoutUnit, LayoutUnit, BesideTabs) {
        let Overhanging {
            space,
            opportunities,
            ruby,
            before,
            after,
        } = column;
        let align = ruby.group.align;
        if align == RubyAlign::SpaceBetween {
            return (LayoutUnit::ZERO, LayoutUnit::ZERO, (None, None));
        }
        // Half the annotation's font size, halved as a whole number of
        // pixels, as Blink takes it: `FontSize() / 2`. `FontSize()` is the
        // computed size rounded to a pixel (`ComputedPixelSize`). So a 17px
        // annotation overhangs by 8, not 8.5 (measured with Chrome 153).
        let half = LayoutUnit::from_px_whole(size).half().floor_px();
        // The base's inset each side under `space-around`, as Chrome's
        // `ComputeRubyBaseInset` takes it.
        let inset = RubySpread::from_base(RubyAlign::SpaceAround, space, opportunities).start;
        let spaces = ruby.group.overhang == RubyOverhang::Spaces;
        let (mut start, mut end) = match (spaces, align) {
            (false, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(half)),
            (false, _) => (inset.min(half), inset.min(half)),
            (true, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(inset + inset)),
            (true, _) => (inset, inset),
        };
        let scan = self.scan;
        let side = RubySide::from_position(ruby.group.position, 0);
        // The text beside may be overhung if it is no larger than the ruby
        // and not marked for emphasis on the annotation's side.
        let may = |beside: &TextBeside| {
            let text = self.item_text(beside.item);
            let facts = scan.input.content.facts.text(text);
            let marked =
                facts.has(TextFlags::EMPHASIS) && RubySide::from(facts.emphasis_side) == side;
            scan.input.size(text) <= ruby.size && !marked
        };
        // Under `spaces` the blank is the bound; otherwise half the text,
        // where no tab stands beside the column.
        let reach = |beside: &TextBeside, forward: bool| {
            if spaces {
                self.blank(beside, forward)
            } else if scan.input.analysis.clusters.is_tab(beside.cluster) {
                Blank::NONE
            } else {
                Blank {
                    width: beside.width.to_layout().half(),
                    tab: None,
                }
            }
        };
        let mut tabs = (None, None);
        start = match before {
            Some(beside) if may(&beside) => {
                let blank = reach(&beside, false);
                tabs.0 = blank.tab;
                start.min(blank.width)
            }
            _ => LayoutUnit::ZERO,
        };
        if spaces && let Some(limit) = self.start_limit {
            start = start.min(limit);
        }
        end = match after {
            Some(beside) if may(&beside) => {
                let blank = reach(&beside, true);
                tabs.1 = blank.tab;
                end.min(blank.width)
            }
            _ => LayoutUnit::ZERO,
        };
        (start, end, tabs)
    }

    /// Returns how much blank the text `beside` a column offers it, as
    /// Chrome's `spaces` reads it.
    ///
    /// It counts from the cluster next to the column, forward into the text
    /// after it (`forward`) or back into the text before. The blank is the
    /// space separators there, then the blank half of a full-width mark
    /// facing the column, or a quarter of a middle dot's. `prefix` holds
    /// sums before the scan's boundary and advances after.
    ///
    /// A tab ends the blank. The prefix gives it no width, and its width
    /// depends on where its line starts. After the column it counts as wide
    /// as a tab under its stops can be. Before it, it counts as wide as on
    /// its paragraph's line, which the scan has passed.
    fn blank(&self, beside: &TextBeside, forward: bool) -> Blank {
        let prefix = self.prefix;
        let cluster = beside.cluster;
        let (content, analysis) = (self.scan.input.content, self.scan.input.analysis);
        let clusters = &analysis.clusters;
        let count = clusters.len();
        // A cluster's advance. Ahead of the scan, `prefix` holds the
        // advance. Behind it, the advance is the difference of two sums,
        // the last of them the scan's own.
        let advance = |at: ClusterId| -> InlineLayoutUnit {
            if forward {
                prefix.get(at).unwrap_or_default()
            } else {
                let from = prefix.get(at).unwrap_or_default();
                let to = if at == cluster {
                    beside.end
                } else {
                    prefix.get(ClusterId::new(at.get() + 1)).unwrap_or(from)
                };
                to - from
            }
        };
        let mut blank = InlineLayoutUnit::ZERO;
        let mut tab = None;
        let mut at = cluster;
        let mark = loop {
            work::step();
            if at.get() >= count {
                break None;
            }
            let class = clusters.attrs(at).map(ClusterAttrs::class);
            if class == Some(ClusterClass::Tab) {
                let width = if forward {
                    let text = self.text(beside.item, at);
                    let stops = self.scan.tab_stops(text);
                    stops.map_or(LayoutUnit::ZERO, |stops| stops.widest())
                } else {
                    self.tab_before
                        .filter(|&(tab, _)| tab == at)
                        .map_or(LayoutUnit::ZERO, |(_, width)| width)
                };
                tab = Some((at, blank.to_layout()));
                blank += InlineLayoutUnit::from_layout(width);
                break None;
            }
            if !matches!(
                class,
                Some(ClusterClass::Space | ClusterClass::OtherSpace | ClusterClass::NoBreakSpace)
            ) {
                break Some(at);
            }
            blank += advance(at);
            at = if forward {
                ClusterId::new(at.get() + 1)
            } else if at.get() == 0 {
                break None;
            } else {
                ClusterId::new(at.get() - 1)
            };
        };
        if let Some(mark) = mark
            && let Some(ch) = clusters.first_char(content.text(self.scan.variant), mark)
        {
            let size = self.size(beside.item, mark);
            let facing = match unicode::rare_props(ch).text_spacing() {
                TextSpacingClass::Open if forward => size.half(),
                TextSpacingClass::Close if !forward => size.half(),
                TextSpacingClass::Middle => size.half().half(),
                _ => LayoutUnit::ZERO,
            };
            // A mark set narrower than full width has no blank half left.
            let full = advance(mark).to_layout() + LayoutUnit::EPSILON >= size - size.divided(10);
            if full {
                blank += InlineLayoutUnit::from_layout(facing);
            }
        }
        Blank {
            width: blank.to_layout(),
            tab,
        }
    }

    /// Returns the computed font size of the text holding `cluster`,
    /// truncated onto the grid as Chrome's `LayoutUnit(font_size)` is.
    ///
    /// The holding item is walked to from `item`, the text beside a column.
    /// Where no item holds it, the block's size is used.
    fn size(&self, item: ItemId, cluster: ClusterId) -> LayoutUnit {
        let text = self.text(item, cluster);
        LayoutUnit::from_px_truncated(self.scan.input.size(text))
    }

    /// Returns the text facts of the item holding `cluster`, found from
    /// `item` as [`size`](Self::size) finds it.
    fn text(&self, item: ItemId, cluster: ClusterId) -> TextFactsId {
        let content = self.scan.input.content;
        let items = &self.scan.input.analysis.item_clusters;
        // The first item at the cluster's start may be a box's edge: the
        // cluster's own item is the first after it that holds clusters.
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

    /// Returns the overhang of the Japanese Layout Requirements (JLREQ
    /// 3.3.7).
    ///
    /// - Under `auto`: over kana only, by at most one annotation em and half
    ///   the difference.
    /// - Under `spaces`: into blank only, by at most half the difference.
    ///   Blank is a space's whole advance, the blank half of a full-width
    ///   mark facing the column, or a quarter of a middle dot's.
    ///
    /// It never reaches over another column, whose annotation is already
    /// there.
    fn jlreq_overhang(
        &self,
        column: Overhanging,
        size: f32,
    ) -> (LayoutUnit, LayoutUnit, BesideTabs) {
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
        let mut tabs = (None, None);
        let mut reach = |beside: Option<TextBeside>, forward: bool| {
            let Some(beside) = beside else {
                return LayoutUnit::ZERO;
            };
            if spaces {
                let blank = self.blank(&beside, forward);
                if forward {
                    tabs.1 = blank.tab;
                } else {
                    tabs.0 = blank.tab;
                }
                return blank.width.min(half);
            }
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
        let start = reach(before, false);
        let end = reach(after, true);
        (start, end, tabs)
    }
}

/// The tabs ending the blank before and after a ruby column, each with the
/// blank between it and the column.
type BesideTabs = (
    Option<(ClusterId, LayoutUnit)>,
    Option<(ClusterId, LayoutUnit)>,
);

/// The blank beside a ruby column, as `ruby-overhang: spaces` reads it.
#[derive(Copy, Clone, Debug)]
struct Blank {
    width: LayoutUnit,
    /// The tab ending it, and the blank between the tab and the column.
    tab: Option<(ClusterId, LayoutUnit)>,
}

impl Blank {
    /// No blank.
    const NONE: Self = Self {
        width: LayoutUnit::ZERO,
        tab: None,
    };
}

/// A column as its overhang reads it: how much wider than its base it is,
/// its container, and the text either side.
#[derive(Copy, Clone, Debug)]
pub(super) struct Overhanging {
    /// How much wider the column is than its base.
    pub(super) space: LayoutUnit,
    /// How many justification opportunities its base has.
    pub(super) opportunities: u32,
    pub(super) ruby: Ruby,
    /// The text before the column and after it, where it can be reached.
    pub(super) before: Option<TextBeside>,
    pub(super) after: Option<TextBeside>,
}

/// A ruby container as a column's overhang reads it: its ruby properties,
/// and its text's computed size.
#[derive(Copy, Clone, Debug)]
pub(super) struct Ruby {
    pub(super) group: RubyGroup,
    pub(super) size: f32,
}

/// The text beside a ruby column, as its overhang reads it.
#[derive(Copy, Clone, Debug)]
pub(super) struct TextBeside {
    pub(super) item: ItemId,
    pub(super) width: InlineLayoutUnit,
    /// The cluster beside the column: the text's last before it, its first
    /// after it.
    pub(super) cluster: ClusterId,
    /// Before the column, the running sum where the text ends, which the
    /// scan's prefix doesn't hold yet. Zero after the column.
    pub(super) end: InlineLayoutUnit,
}
