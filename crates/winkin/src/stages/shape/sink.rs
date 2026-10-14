//! Encodes shaped glyphs into cluster words, sidecar glyphs and advances.

use alloc::vec::Vec;
use core::slice;

use harfrust::{GlyphInfo, GlyphPosition};

use super::{ClusterGlyphs, CombineFit, GlyphStore, GlyphWord, SidecarGlyph, SidecarGlyphId};
use crate::data::{Id, Table, heap_bytes, index_to_u32, u32_to_index};
use crate::stages::analysis::ClusterId;
use crate::unit::{LayoutUnit, TextUnit};
use crate::work;

/// Where a [`GlyphSink`] appends its advances, one for each word it keeps.
///
/// Each advance is the sum of the cluster's glyph advances along the line,
/// in 16.16. A cluster wider than 32,767 px saturates.
///
/// The shaping pass uses a list, which measurement sums into 48.16 prefix
/// sums. Lines' reshaped edges use a table indexed like the words.
pub(crate) trait AdvanceStore {
    /// Appends the advance of the word just kept.
    fn push_advance(&mut self, advance: TextUnit);
}

impl AdvanceStore for Vec<TextUnit> {
    #[inline]
    fn push_advance(&mut self, advance: TextUnit) {
        self.push(advance);
    }
}

impl<I: Id> AdvanceStore for Table<I, TextUnit> {
    /// Pushed right after the word, into a table with the same id type that
    /// is never longer than the words', so it has room too.
    #[inline]
    fn push_advance(&mut self, advance: TextUnit) {
        let kept = self.push(advance);
        debug_assert!(kept.is_some(), "an advance a word, which the words took");
    }
}

/// Where [`shape_range`](super::shape_range) writes, one cluster at a time.
///
/// It writes a word and an advance per cluster, and the glyphs of expanded
/// clusters. The tables are the shaped text's for the shaping pass, or the
/// reshaped edges' for the breaker, each with its own id type and
/// [`AdvanceStore`]. A cluster the words table cannot take gets neither.
pub(crate) struct GlyphSink<'a, I: Id, S: AdvanceStore = Vec<TextUnit>> {
    words: &'a mut Table<I, GlyphWord>,
    /// The first word this sink writes.
    first: I,
    sidecar: &'a mut Table<SidecarGlyphId, SidecarGlyph>,
    /// Each cluster's advance along the line.
    advances: &'a mut S,
    unsafe_seen: bool,
    dropped: usize,
    /// Whether every cluster is written expanded, with its glyphs' advances
    /// in the sidecar, as the method `keep_advances` sets.
    keep_advances: bool,
    /// Where a cluster expanded to one glyph finds a sidecar entry already
    /// holding that glyph, to point at rather than write again.
    ///
    /// Only the shaping pass's sink has it, never a line's, and uses it only
    /// while `share` allows.
    shared: Option<&'a mut SharedGlyphs>,
    share: bool,
}

/// The sidecar entries of a paragraph's one-glyph clusters, found again by
/// glyph id.
///
/// Upright text in a vertical line expands every glyph for its offsets, and
/// draws a few hundred glyphs thousands of times. Each cluster drawing one
/// of them points at the one entry holding it.
///
/// A glyph id's low bits pick a slot, which keeps the entry last written for
/// such a glyph. An entry is shared only where it holds exactly the glyph a
/// cluster draws: its id, offsets and advance. A slot another glyph took, or
/// one pointing past the sidecar's end, only means writing the glyph again.
/// The context owns it and keeps it between builds only for its capacity.
///
/// Slots are filled the first time a cluster asks for one, not on clear.
/// Most text expands no cluster to one glyph, and a fill writes 8 KiB, which
/// a small layout would otherwise pay on every build.
pub(super) struct SharedGlyphs {
    /// Every slot's sidecar entry, `u32::MAX` for none, or no slots since the
    /// last [`clear`](Self::clear). A sidecar entry's id is below 2^28.
    slots: Vec<u32>,
}

impl SharedGlyphs {
    /// How many slots there are: a power of two, and more than the distinct
    /// glyphs a paragraph of Japanese draws.
    const SLOTS: usize = 2048;

    /// Returns empty slots, allocating nothing until the first use.
    pub(super) const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Forgets every entry, for a sidecar written from its start, keeping
    /// the slots' capacity.
    pub(super) fn clear(&mut self) {
        self.slots.clear();
    }

    /// Returns the slot for glyph `id`, first filling every slot with none
    /// if none has been made since the last clear.
    fn slot(&mut self, id: u32) -> Option<&mut u32> {
        if self.slots.is_empty() {
            self.slots.resize(Self::SLOTS, u32::MAX);
        }
        let at = usize::try_from(id).unwrap_or(0) & (Self::SLOTS - 1);
        self.slots.get_mut(at)
    }
}

heap_bytes! {
    SharedGlyphs { slots }
}

impl<'a, I: Id, S: AdvanceStore> GlyphSink<'a, I, S> {
    /// Returns a sink that appends to `words`, `sidecar` and `advances`.
    pub(crate) fn new(
        words: &'a mut Table<I, GlyphWord>,
        sidecar: &'a mut Table<SidecarGlyphId, SidecarGlyph>,
        advances: &'a mut S,
    ) -> Self {
        Self {
            first: words.next_id(),
            words,
            sidecar,
            advances,
            unsafe_seen: false,
            dropped: 0,
            keep_advances: false,
            shared: None,
            share: false,
        }
    }

    /// Returns the sink, pointing a cluster expanded to one glyph at an entry
    /// `shared` finds already holding it ([`SharedGlyphs`]).
    ///
    /// It shares only while [`share`](Self::share) allows.
    pub(super) fn with_shared(self, shared: &'a mut SharedGlyphs) -> Self {
        Self {
            shared: Some(shared),
            ..self
        }
    }

    /// Sets whether later clusters point at shared entries or each write
    /// their own.
    ///
    /// Combined text writes its own, since its glyphs are moved into its em
    /// once written.
    pub(super) fn share(&mut self, share: bool) {
        self.share = share;
    }

    /// Sets whether later clusters are written expanded, with their glyphs'
    /// advances in the sidecar.
    ///
    /// Ruby annotation text needs this: line layout sets it on a line of its
    /// own by its glyphs' advances, which the prefix sums lack. Combined
    /// text needs it to set its glyphs across its em.
    pub(super) fn keep_advances(&mut self, keep: bool) {
        self.keep_advances = keep;
    }

    /// Returns whether any cluster written was unsafe to break before.
    pub(super) fn any_unsafe(&self) -> bool {
        self.unsafe_seen
    }

    /// Returns how many words it has written: one per cluster the words
    /// table took.
    pub(crate) fn words_written(&self) -> usize {
        self.words.next_id().get().saturating_sub(self.first.get())
    }

    /// Returns how many glyphs were left out.
    ///
    /// Glyphs are left out past what a word can point into the sidecar,
    /// 2^28 − 1 glyphs, or past what the sidecar can take. Their cluster is
    /// written with no glyphs and keeps its advance. A cluster the words
    /// table cannot take counts as one, with no word and no advance.
    pub(crate) fn dropped(&self) -> usize {
        self.dropped
    }

    /// Copies `cluster` from `glyphs`, with an advance of `advance`
    /// 1/65536 px.
    ///
    /// This serves a line's last cluster that gives a seam's room back,
    /// copied rather than shaped again, as Chrome's `UnapplyAutoSpacing`
    /// copies its line's last glyph. Its glyphs go into this sink's sidecar,
    /// and its word keeps its flags.
    pub(crate) fn copy(&mut self, glyphs: &GlyphStore, cluster: ClusterId, advance: i64) {
        let word = glyphs.word(cluster);
        let copied = match word.glyphs(&glyphs.sidecar) {
            ClusterGlyphs::Many(glyphs) => {
                let moved = word.with_sidecar_start(self.sidecar.next_id());
                match moved.filter(|_| self.sidecar.remaining() >= glyphs.len()) {
                    Some(moved) => {
                        for glyph in glyphs {
                            self.sidecar.push_bounded(*glyph, "room was checked");
                        }
                        moved
                    }
                    None => {
                        self.dropped = self.dropped.saturating_add(glyphs.len());
                        GlyphWord::EMPTY
                    }
                }
            }
            ClusterGlyphs::One(_) | ClusterGlyphs::None => word,
        };
        self.push(copied, advance);
    }

    /// Writes `count` clusters with no glyphs and no advance.
    pub(super) fn empty(&mut self, count: usize) {
        for _ in 0..count {
            self.push(GlyphWord::EMPTY, 0);
        }
    }

    fn push(&mut self, word: GlyphWord, advance: i64) {
        if self.words.push(word).is_some() {
            self.advances
                .push_advance(TextUnit::from_raw(saturate(advance)));
            self.unsafe_seen |= word.is_unsafe_to_break();
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Writes a cluster drawn by `glyph` alone at `position`, across a
    /// horizontal line and untrimmed, with `flags`.
    ///
    /// It writes as [`cluster`](Self::cluster) does, trying the compact form
    /// first.
    #[inline]
    pub(super) fn one(&mut self, glyph: &GlyphInfo, position: &GlyphPosition, flags: Flags) {
        if let Some(word) = self.compact(glyph, position) {
            self.push(flags.apply(word), i64::from(position.x_advance));
            return;
        }
        let (glyphs, positions) = (slice::from_ref(glyph), slice::from_ref(position));
        self.cluster(glyphs, positions, false, flags, Halved::default());
    }

    /// Writes a cluster drawn by `glyphs`, shaped `vertical`ly or not, with
    /// `flags`, moved as `halved` says.
    pub(super) fn cluster(
        &mut self,
        glyphs: &[GlyphInfo],
        positions: &[GlyphPosition],
        vertical: bool,
        flags: Flags,
        halved: Halved,
    ) {
        let sum: i64 = positions
            .iter()
            .map(|position| along(position, vertical))
            .sum();
        let word = match (glyphs, positions) {
            ([], _) | (_, []) => GlyphWord::EMPTY,
            // Compact: one glyph, drawn where the pen is, along a
            // horizontal line.
            ([glyph], [position]) if !vertical && halved.shift == 0 => {
                match self.compact(glyph, position) {
                    Some(word) => word,
                    None => self.expand(glyphs, positions, vertical, halved),
                }
            }
            _ => self.expand(glyphs, positions, vertical, halved),
        };
        let taken = i64::from(halved.shift) + i64::from(halved.trim);
        self.push(flags.apply(word), sum + taken);
    }

    /// Returns the compact word of a cluster drawn by `glyph` alone at
    /// `position` along a horizontal line, or `None` where it is written
    /// expanded.
    ///
    /// It is expanded where the sink keeps advances, where the glyph is
    /// drawn off its pen, or where a word cannot hold its id.
    #[inline]
    fn compact(&self, glyph: &GlyphInfo, position: &GlyphPosition) -> Option<GlyphWord> {
        if self.keep_advances || position.x_offset != 0 || position.y_offset != 0 {
            return None;
        }
        GlyphWord::from_glyph(glyph.glyph_id)
    }

    /// Writes `glyphs` to the sidecar, shaped `vertical`ly or not and moved
    /// as `halved` says, and returns the word that points at them, or
    /// [`GlyphWord::EMPTY`], counting them dropped, where they do not fit.
    ///
    /// Each glyph's offsets are kept in the line's own terms, along it and
    /// over it. For text shaped across, these are harfrust's right and up.
    /// For text shaped down the line, harfrust's down is along it and its
    /// right is over it, since a vertical line's over side is its right
    /// (CSS Writing Modes 3, section 6.4). So every reader places a glyph
    /// the same way, whichever way its run was shaped.
    fn expand(
        &mut self,
        glyphs: &[GlyphInfo],
        positions: &[GlyphPosition],
        vertical: bool,
        halved: Halved,
    ) -> GlyphWord {
        let count = glyphs.len().min(positions.len());
        if count == 1
            && self.share
            && let (Some(glyph), Some(position)) = (glyphs.first(), positions.first())
        {
            return self.expand_one(glyph, position, vertical, halved);
        }
        let word = GlyphWord::from_sidecar(self.sidecar.next_id());
        let Some(word) = word.filter(|_| self.sidecar.remaining() >= count) else {
            self.dropped = self.dropped.saturating_add(count);
            return GlyphWord::EMPTY;
        };
        for (at, (glyph, position)) in glyphs.iter().zip(positions).enumerate() {
            let placed = sidecar_glyph(glyph, position, vertical, halved, at + 1 == count);
            self.sidecar.push_bounded(placed, "room was checked");
        }
        word
    }
}

impl<I: Id, S: AdvanceStore> GlyphSink<'_, I, S> {
    /// Writes a cluster of one glyph, `glyph` at `position`, as
    /// [`expand`](Self::expand) does.
    ///
    /// It points at a sidecar entry already holding that glyph where one is
    /// found, and writes the glyph otherwise.
    fn expand_one(
        &mut self,
        glyph: &GlyphInfo,
        position: &GlyphPosition,
        vertical: bool,
        halved: Halved,
    ) -> GlyphWord {
        let written = sidecar_glyph(glyph, position, vertical, halved, true);
        if let Some(held) = self.shared_slot(glyph.glyph_id).map(|slot| *slot)
            && let Some(held) = (held != u32::MAX).then(|| SidecarGlyphId::new(u32_to_index(held)))
            && self.sidecar.get(held) == Some(&written)
            && let Some(word) = GlyphWord::from_sidecar(held)
        {
            return word;
        }
        let at = self.sidecar.next_id();
        let word = self.write_one(written);
        if word != GlyphWord::EMPTY
            && let Some(slot) = self.shared_slot(glyph.glyph_id)
        {
            *slot = index_to_u32(at.get());
        }
        word
    }

    /// Returns the [`SharedGlyphs`] slot for glyph `id`, or `None` where the
    /// sink has no shared entries.
    fn shared_slot(&mut self, id: u32) -> Option<&mut u32> {
        self.shared.as_mut().and_then(|shared| shared.slot(id))
    }

    /// Writes `glyph` to the sidecar as a cluster's one glyph, and returns
    /// the word pointing at it, or [`GlyphWord::EMPTY`], counting it
    /// dropped, where it does not fit.
    fn write_one(&mut self, glyph: SidecarGlyph) -> GlyphWord {
        let Some(word) = GlyphWord::from_sidecar(self.sidecar.next_id()) else {
            self.dropped = self.dropped.saturating_add(1);
            return GlyphWord::EMPTY;
        };
        if self.sidecar.push(glyph).is_none() {
            self.dropped = self.dropped.saturating_add(1);
            return GlyphWord::EMPTY;
        }
        word
    }
}

/// How far a glyph placed at `position` moves the pen along the line, in
/// 16.16.
///
/// That is across, or down where it was shaped `vertical`ly. The shaper
/// gives down as a negative `y`.
#[inline]
pub(super) fn along(position: &GlyphPosition, vertical: bool) -> i64 {
    if vertical {
        -i64::from(position.y_advance)
    } else {
        i64::from(position.x_advance)
    }
}

/// Returns `glyph` at `position` as the sidecar keeps it.
///
/// The glyph is shaped `vertical`ly or not, moved as `halved` says, and is
/// its cluster's last where `last`. Its offsets are in the line's terms (see
/// [`GlyphSink::expand`]). The last glyph's advance includes the cluster's
/// trim.
pub(super) fn sidecar_glyph(
    glyph: &GlyphInfo,
    position: &GlyphPosition,
    vertical: bool,
    halved: Halved,
    last: bool,
) -> SidecarGlyph {
    let taken = if last {
        i64::from(halved.shift) + i64::from(halved.trim)
    } else {
        0
    };
    let (across, over) = if vertical {
        (position.y_offset.saturating_neg(), position.x_offset)
    } else {
        (position.x_offset, position.y_offset)
    };
    SidecarGlyph {
        id: (glyph.glyph_id & !SidecarGlyph::LAST) | if last { SidecarGlyph::LAST } else { 0 },
        x_offset: TextUnit::from_raw(across.saturating_add(halved.shift)),
        y_offset: TextUnit::from_raw(over),
        advance: TextUnit::from_raw(saturate(along(position, vertical) + taken)),
    }
}

/// A position in a shaping sink's tables.
///
/// A combined unit shaped again in a narrower form rewinds to it.
#[derive(Copy, Clone, Debug)]
pub(super) struct SinkMark {
    words: ClusterId,
    sidecar: SidecarGlyphId,
    dropped: usize,
}

impl GlyphSink<'_, ClusterId> {
    /// Returns where the tables stand.
    pub(super) fn mark(&self) -> SinkMark {
        SinkMark {
            words: self.words.next_id(),
            sidecar: self.sidecar.next_id(),
            dropped: self.dropped,
        }
    }

    /// Removes everything written since `mark`.
    pub(super) fn rewind(&mut self, mark: SinkMark) {
        self.words.truncate(mark.words);
        self.sidecar.truncate(mark.sidecar);
        self.advances.truncate(mark.words.get());
        self.dropped = mark.dropped;
    }

    /// Returns how far the glyphs written since `mark` reach across as
    /// horizontal text, in 1/65536 px.
    ///
    /// This is the combined unit's width, as Blink's
    /// `CalculateWidthForTextCombine` sums it.
    pub(super) fn width_since(&self, mark: SinkMark) -> i64 {
        self.sidecar
            .get_slice(mark.sidecar..self.sidecar.next_id())
            .unwrap_or_default()
            .iter()
            .map(|glyph| i64::from(glyph.advance.raw()))
            .sum()
    }

    /// Sets the combined unit written since `mark` into its one em, `em`
    /// along the line, as `fit` says.
    ///
    /// The unit is shaped as horizontal text, then set upright:
    /// - their baseline sits `baseline` down the em;
    /// - they are laid across the em from its left, narrowed by the fit's
    ///   scale, and centred on the fit's middle over the line's baseline;
    /// - their offsets stay in the line's terms (see `expand`).
    ///
    /// Only the last cluster with glyphs advances, by the em. So every
    /// glyph's pen is the unit's start, whichever cluster draws it. The last
    /// glyph's advance is the em, so the clusters' glyphs still add up to
    /// their advances.
    pub(super) fn set_combined(
        &mut self,
        mark: SinkMark,
        em: LayoutUnit,
        baseline: LayoutUnit,
        fit: CombineFit,
    ) {
        let scale = fit.scale();
        let width = TextUnit::from_raw(saturate(self.width_since(mark))).to_px() * scale;
        // Where the text's left edge is over the baseline: under it by half
        // the text, then over it by the middle.
        let left = fit.middle.to_px() - width / 2.0;
        let baseline = TextUnit::from_px(baseline.to_px());
        let glyphs = self
            .sidecar
            .get_slice_mut(mark.sidecar..self.sidecar.next_id())
            .unwrap_or_default();
        let mut pen = 0i64;
        for glyph in glyphs.iter_mut() {
            work::step();
            let across =
                TextUnit::from_raw(saturate(pen + i64::from(glyph.x_offset.raw()))).to_px();
            pen += i64::from(glyph.advance.raw());
            // Down the em to the baseline, less the glyph's own raise.
            glyph.x_offset =
                TextUnit::from_raw(baseline.raw().saturating_sub(glyph.y_offset.raw()));
            glyph.y_offset = TextUnit::from_px(left + across * scale);
            glyph.advance = TextUnit::from_raw(0);
        }
        let em_along = TextUnit::from_px(em.to_px());
        if let Some(last) = glyphs.last_mut() {
            last.advance = em_along;
        }
        // The cluster drawing the last glyph takes the em; with no glyph at
        // all, the unit's last.
        let words = self
            .words
            .get_slice(mark.words..self.words.next_id())
            .unwrap_or_default();
        let sidecar = &*self.sidecar;
        let carrier = words
            .iter()
            .rposition(|word| matches!(word.glyphs(sidecar), ClusterGlyphs::Many(_)))
            .or(words.len().checked_sub(1));
        let unit = self
            .advances
            .get_mut(mark.words.get()..)
            .unwrap_or_default();
        for (at, advance) in unit.iter_mut().enumerate() {
            work::step();
            *advance = if Some(at) == carrier {
                em_along
            } else {
                TextUnit::from_raw(0)
            };
        }
    }
}

/// How a mark trimmed by halving its advance moves.
///
/// - An opening mark, whose blank is before its ink, moves its glyphs back
///   by `shift` and shortens its advance by the same.
/// - A closing mark, whose blank is after, shortens its advance by `trim`.
///
/// Both are negative, in 16.16, and zero for any other cluster.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Halved {
    pub(super) shift: i32,
    pub(super) trim: i32,
}

/// Fits `sum`, a glyph's advance or a cluster's 16.16 sum, in an `i32`,
/// saturating.
fn saturate(sum: i64) -> i32 {
    i32::try_from(sum).unwrap_or(if sum < 0 { i32::MIN } else { i32::MAX })
}

/// A cluster's unsafe bit and whether it continues the glyphs before it.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Flags {
    pub(super) unsafe_to_break: bool,
    pub(super) continuation: bool,
}

impl Flags {
    fn apply(self, word: GlyphWord) -> GlyphWord {
        let word = word.with_unsafe(self.unsafe_to_break);
        if self.continuation {
            word.continuing()
        } else {
            word
        }
    }
}
