//! Bidi levels for analysis: each paragraph's input, its resolution, and
//! where its clusters' levels change.
//!
//! In: a paragraph the writer has finished. Out: its base level and flags,
//! the level changes and the control levels in [`BidiInput`].
//! Start at: [`resolve`], then [`bidi_units`].
//!
//! - `input` builds the units: one per cluster and per control a style
//!   synthesized, in order. The levels go back to the clusters in order,
//!   skipping the controls, so no second array is kept in step.
//! - `resolver` resolves them (UAX #9).
//!
//! - **A cluster** is one unit, of its first character's class. Two
//!   exceptions keep it to the per-character answer, which
//!   `tests/bidi_conformance.rs` checks. Whitespace or a separator carrying
//!   a mark resolves as the pair would (`unicode::cluster_bidi_class`).
//!   Upright text in a vertical line, and text `text-combine-upright: all`
//!   combines, is left to right with no brackets.
//! - **Controls come from styles**, never from the text, as Blink injects
//!   them (`InlineItemsBuilder::EnterInline`). The block's own override opens
//!   every paragraph, as Blink's `EnterBlock` pushes it. A ruby container
//!   adds its `unicode-bidi`'s controls, as a box does. A ruby annotation is
//!   a first-strong isolate, so it neither moves nor is moved by its base,
//!   and its own `unicode-bidi` adds controls inside it. A float is a
//!   neutral, as Blink resolves its U+FFFC. The block's initial letter is
//!   an isolate, so it resolves as the neutral U+FFFC Blink sets it as.
//! - **A paragraph re-opens** the controls of every box open across its
//!   start, as Blink does around a forced break (CSS Writing Modes 3, section
//!   2.4.3). `track` keeps the open boxes as the writer walks the items
//!   ([`BidiInput::walk`]), so a paragraph never walks up its ancestors.
//! - **A bidi paragraph may end inside one**, after U+001C, U+001D or
//!   U+001E, which are class B but break no line. What follows is resolved
//!   on its own ([`BidiInput::resolve_units`]), with the style controls
//!   re-opened (CSS Writing Modes 4, section 2.4.4).
//!
//! **A fast path** skips the resolver. A paragraph with no right-to-left
//! class and no right-to-left control, in a block that is not right to
//! left, is not walked: every cluster is at level 0, as Blink sets a
//! unidirectional paragraph. A paragraph whose clusters all sit at its own
//! level records no changes.
//!
//! **Nothing here panics.** The resolver takes any nesting depth, keeping to
//! UAX #9's 125 levels, and any balance of controls.

mod input;
mod resolver;

use super::classify::LEVEL_CLASSES;
use super::{Analysis, BidiLevel, ClusterId, ParagraphFlags, ParagraphState};
use crate::data::Id;
use crate::stages::content::{Content, ItemId};
use crate::unicode::BidiClass;
use crate::work;
use input::{ItemControls, bidi_units};

pub(super) use input::BidiInput;
#[cfg(test)]
pub(super) use resolver::{BidiScratch, BidiUnit, reset_separators, resolve_bidi};

/// The level of the bidi controls a box's open or close item adds, where
/// they sit below the clusters on both sides.
///
/// Blink reorders a line with a level-only item for each control
/// (`LogicalLineBuilder::HandleItemResults`). A control at or above the
/// lower of its neighbours' levels leaves the clusters' visual order as it
/// is. So only these are kept, and line layout reorders with each of them.
/// An example is `R LRI L PDI` at level 0: the LRI at 0 keeps the R at 1
/// and the L at 2 from reversing together.
///
/// An absolutely positioned box's anchor is resolved as a U+FFFC, as
/// Chrome's `InlineNode::SegmentBidiRuns` resolves it. Its level is kept
/// whatever its neighbours' are, since it places the anchor on its line.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ControlLevel {
    /// The open or close item that adds the controls, or the anchor.
    pub(crate) item: ItemId,
    /// The lowest level among its controls that X9 keeps.
    pub(crate) level: BidiLevel,
}

/// The classes that keep a right-to-left paragraph off level 1 throughout:
/// L, the numbers, and every explicit control.
const NOT_PURE_RTL: u32 = BidiClass::LEFT_TO_RIGHT.mask()
    | BidiClass::EUROPEAN_NUMBER.mask()
    | BidiClass::ARABIC_NUMBER.mask()
    | BidiClass::LEFT_TO_RIGHT_EMBEDDING.mask()
    | BidiClass::LEFT_TO_RIGHT_OVERRIDE.mask()
    | BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::POP_DIRECTIONAL_FORMAT.mask()
    | BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
    | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
    | BidiClass::FIRST_STRONG_ISOLATE.mask()
    | BidiClass::POP_DIRECTIONAL_ISOLATE.mask();

/// Resolves `paragraph`, whose clusters end at the last one `analysis`
/// holds, and returns its base level and flags.
///
/// `requested` is the level the block's direction sets, or `None` under
/// `auto`.
///
/// If some cluster's level differs from the paragraph's, this records the
/// changes in `input` and sets `MIXED_LEVELS`. It sets `RIGHT_TO_LEFT` if
/// the paragraph or any cluster reads right to left.
pub(super) fn resolve(
    content: &Content,
    analysis: &Analysis,
    input: &mut BidiInput,
    paragraph: &ParagraphState,
    requested: Option<BidiLevel>,
) -> (BidiLevel, ParagraphFlags) {
    let range = paragraph.start..analysis.clusters.end_id();
    // A paragraph at `base` whose clusters all are.
    let own = |base: u8| {
        let level = BidiLevel::new(base);
        let flags = if level.is_rtl() {
            ParagraphFlags::RIGHT_TO_LEFT
        } else {
            ParagraphFlags::NONE
        };
        (level, flags)
    };
    let requested = requested.map(BidiLevel::get);
    // Nothing to resolve: its own level, which with nothing right to left
    // in it is 0 under `auto` too. Left-to-right controls raise only even
    // levels, so Blink finds such a paragraph unidirectional and sets every
    // item at 0 (`InlineNode::SegmentBidiRuns`).
    let base = requested.unwrap_or(0);
    if !(paragraph.may_differ() || requested == Some(1)) || range.is_empty() {
        return own(base);
    }
    let mask = bidi_units(
        content,
        &analysis.clusters,
        &analysis.item_clusters,
        range.clone(),
        paragraph.first_item,
        input,
    );
    // Nothing right to left, which upright text and the units' classes can
    // leave where the characters had some.
    if requested != Some(1) && mask & LEVEL_CLASSES == 0 {
        return own(0);
    }
    // Nothing left to right, no number and no control, in a paragraph that
    // is right to left, or is under `auto` with no bidi paragraph ending
    // inside it: every class resolves to R at level 1, and L1 sets
    // separators and white space at 1 too.
    if mask & NOT_PURE_RTL == 0
        && (requested == Some(1) || requested.is_none() && input.splits.is_empty())
    {
        return own(1);
    }
    let (level, flags) = input.resolve_units(requested, mask, range.start);
    if flags.contains(ParagraphFlags::MIXED_LEVELS.union(ParagraphFlags::RIGHT_TO_LEFT)) {
        input.keep_control_levels();
    }
    (level, flags)
}

impl BidiInput {
    /// Resolves the units into `levels`, one per unit, records where the
    /// clusters' levels change from `first` on, and returns the paragraph's
    /// base level and flags.
    ///
    /// `requested` is the base level the block asks for, or `None` under
    /// `auto`. `mask` holds every class among the units.
    ///
    /// Each bidi paragraph is resolved apart, with its own base level under
    /// `auto` and its own brackets ([`splits`](Self::splits)). Nearly always
    /// there is one, and this is one call to the resolver. Its levels are
    /// finished and handed out in the same pass, which records a change at
    /// the first cluster and at each whose level is not the one before's.
    /// If every cluster sits at the base level, no change is kept.
    fn resolve_units(
        &mut self,
        requested: Option<u8>,
        mask: u32,
        first: ClusterId,
    ) -> (BidiLevel, ParagraphFlags) {
        let Self {
            units,
            brackets,
            splits,
            levels,
            changes,
            resolver,
            ..
        } = self;
        levels.clear();
        levels.resize(units.len(), 0);
        let kept = changes.len();
        let mut paragraph = None;
        let (mut cluster, mut current, mut differs, mut rtl) = (first, None, false, false);
        let mut bracket_from = 0;
        let ends = splits.iter().map(|&at| at as usize).chain([units.len()]);
        let mut from = 0;
        for to in ends {
            work::step();
            // Its brackets, counted from its start: they are in order, and
            // those before it were counted from the parts before.
            let start = from as u32;
            let mut bracket_to = bracket_from;
            while let Some(bracket) = brackets
                .get_mut(bracket_to)
                .filter(|bracket| (bracket.index as usize) < to)
            {
                bracket.index -= start;
                bracket_to += 1;
            }
            let part = units.get(from..to).unwrap_or_default();
            let part_levels = levels.get_mut(from..to).unwrap_or_default();
            let base = resolver::resolve_bidi(
                resolver,
                part,
                brackets.get(bracket_from..bracket_to).unwrap_or_default(),
                mask,
                requested,
                part_levels,
            );
            let own = *paragraph.get_or_insert(base);
            resolver::reset_separators(part, part_levels, base, |at, level| {
                if part.get(at).is_some_and(|unit| unit.control) {
                    return;
                }
                let level = BidiLevel::new(level);
                if current != Some(level) {
                    changes.push((cluster, level));
                    current = Some(level);
                    rtl |= level.is_rtl();
                }
                differs |= level.get() != own;
                cluster = ClusterId::new(cluster.get() + 1);
            });
            from = to;
            bracket_from = bracket_to;
        }
        let level = BidiLevel::new(paragraph.unwrap_or(requested.unwrap_or(0)));
        let mut flags = ParagraphFlags::NONE;
        if differs {
            flags.insert(ParagraphFlags::MIXED_LEVELS);
        } else {
            changes.truncate(kept);
            rtl = false;
        }
        if rtl || level.is_rtl() {
            flags.insert(ParagraphFlags::RIGHT_TO_LEFT);
        }
        (level, flags)
    }
}

impl BidiInput {
    /// Keeps the level of each box's controls that sit below the clusters
    /// on both sides, and of every anchor, in `control_levels`.
    ///
    /// One forward walk over the units: the controls met since the last
    /// cluster are settled at the next cluster. Controls with no cluster
    /// on one side sit at a line's edge or the paragraph's, where no
    /// level splits anything. An anchor is kept wherever it sits. The units
    /// stop at the paragraph's last cluster ([`bidi_units`]), so no control
    /// comes after it.
    fn keep_control_levels(&mut self) {
        let Self {
            units,
            levels,
            item_controls,
            control_levels,
            ..
        } = self;
        // The lowest level among `controls`' units that X9 keeps.
        let lowest = |controls: &ItemControls| {
            let from = controls.start as usize;
            let to = controls.end as usize;
            (units.get(from..to).unwrap_or_default().iter())
                .zip(levels.get(from..to).unwrap_or_default())
                .filter(|(unit, _)| !unit.class.is_removed_by_x9())
                .map(|(_, &level)| level)
                .min()
        };
        let mut rest = item_controls.as_slice();
        let mut left: Option<u8> = None;
        for (at, (unit, &right)) in units.iter().zip(levels.iter()).enumerate() {
            work::step();
            if unit.control {
                continue;
            }
            let at = u32::try_from(at).unwrap_or(u32::MAX);
            while let Some((controls, tail)) = rest.split_first()
                && controls.end <= at
            {
                rest = tail;
                let Some(level) = lowest(controls) else {
                    continue;
                };
                if controls.anchor || left.is_some_and(|left| level < left.min(right)) {
                    control_levels.push(ControlLevel {
                        item: controls.item,
                        level: BidiLevel::new(level),
                    });
                }
            }
            left = Some(right);
        }
    }
}
