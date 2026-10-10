//! Each text item's em box, recorded at segment starts beside the item
//! extents.

use super::scan::Scan;
use super::{Extent, MeasuredText, ruby};
use crate::data::IdRange;
use crate::stages::Segment;
use crate::stages::content::{ContentFlags, ItemFlags, ItemId, ItemKind};
use crate::stages::fonts::UsedFontId;

/// The em box of the text item the scan is in, built segment by segment.
///
/// It lives apart from the copyable walk, since a ruby look-ahead writes no
/// em boxes.
pub(super) struct ItemEmScan {
    /// The item the em box is for.
    item: Option<ItemId>,
    /// The union of the em boxes of the item's fonts so far.
    em: Extent,
    /// Every font so far has the same em box.
    uniform: bool,
    /// The last font that widened `em`.
    font: Option<UsedFontId>,
}

impl ItemEmScan {
    /// Starts the em boxes in `out`, or returns `None` where the content has
    /// neither ruby nor emphasis marks.
    pub(super) fn new(scan: &Scan<'_>, out: &mut MeasuredText) -> Option<Self> {
        let flags = scan.input.content.flags;
        if !flags.contains(ContentFlags::RUBY) && !flags.contains(ContentFlags::EMPHASIS) {
            return None;
        }
        out.rare_mut().em_boxes.start();
        Some(Self {
            item: None,
            em: Extent::NONE,
            uniform: true,
            font: None,
        })
    }

    /// Enters `segment`, finishing the last item where it starts a new one,
    /// and widens the em box by the segment's font.
    pub(super) fn enter(&mut self, scan: &Scan<'_>, segment: &Segment, out: &mut MeasuredText) {
        if self.item != Some(segment.item) {
            self.flush(scan, out);
            let boxes = &mut out.rare_mut().em_boxes;
            while boxes.items.next_id() < segment.item {
                boxes.push(None);
            }
            self.item = Some(segment.item);
            self.em = Extent::NONE;
            self.uniform = true;
            self.font = None;
        }
        let font = scan.font(segment);
        if font.is_some() && font == self.font {
            return;
        }
        self.font = font;
        let Some(used) = font.and_then(|font| scan.input.fonts.used.get(font)) else {
            self.uniform = false;
            return;
        };
        let own = Extent::new(
            used.metrics.em_over.ceil_px(),
            used.metrics.em_under.ceil_px(),
        );
        self.uniform &= self.em.is_none() || self.em == own;
        self.em = self.em.unite(own);
    }

    /// Pushes the current item's em box to `out`.
    ///
    /// Only base text set in fonts with one em box has one. It is capped by
    /// the primary font's ascent and descent.
    fn flush(&mut self, scan: &Scan<'_>, out: &mut MeasuredText) {
        let Some(id) = self.item.take() else {
            return;
        };
        let em = match scan.input.content.items.get(id) {
            Some(item)
                if item.kind == ItemKind::Text
                    && !item.flags.contains(ItemFlags::ANNOTATION)
                    && self.uniform
                    && !self.em.is_none() =>
            {
                let text = scan.input.content.nodes.text_facts(item.node, scan.variant);
                scan.input.primary(text).map(|cap| {
                    Extent::new(
                        self.em.ascent().min(cap.ascent),
                        self.em.descent().min(cap.descent),
                    )
                })
            }
            _ => None,
        };
        out.rare_mut().em_boxes.push(em);
    }

    /// Pushes the last item's em box, and those of the items the variant's
    /// shaping doesn't reach.
    pub(super) fn finish(mut self, scan: &Scan<'_>, out: &mut MeasuredText) {
        // First-line shaping can stop inside an item. Its stored em box still
        // describes the whole item, including fonts past that paragraph.
        let partial = self
            .item
            .filter(|&id| scan.input.analysis.item_clusters.range(id).end > scan.end);
        if partial.is_none() {
            self.flush(scan, out);
        }
        let first = out.rare_mut().em_boxes.items.next_id();
        if first >= scan.input.content.items.next_id()
            || scan.input.analysis.item_clusters.range(first).start > scan.end
        {
            return;
        }
        let text_end = scan.input.analysis.clusters.end_id();
        let runs = scan.input.fonts.runs(scan.variant);
        // Seek once where the unshaped tail starts, then walk the fonts in
        // order.
        let mut run = runs.cursor_containing(
            scan.input.analysis.item_clusters.range(first).start,
            text_end,
        );
        for id in (first..scan.input.content.items.next_id()).ids() {
            let range = scan.input.analysis.item_clusters.range(id);
            if range.start > scan.end {
                break;
            }
            let Some(item) = scan.input.content.items.get(id) else {
                break;
            };
            let text = item.kind == ItemKind::Text
                && !range.is_empty()
                && !item.flags.contains(ItemFlags::ANNOTATION);
            let em = if text && let Some(run) = &mut run {
                runs.step_to(run, range.start, text_end);
                let text = scan.input.content.nodes.text_facts(item.node, scan.variant);
                let request = scan.input.content.facts.text_request(text);
                let (em, uniform) =
                    ruby::em_box(scan.input.fonts, request, range, scan.variant, run.id());
                uniform.then_some(em)
            } else {
                None
            };
            out.rare_mut().em_boxes.push(em);
        }
    }
}
