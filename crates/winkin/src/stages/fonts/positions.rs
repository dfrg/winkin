//! Which characters a font's `sups` or `subs` draws, for
//! `font-variant-position`.
//!
//! CSS Fonts 4, section 6.5, says that "if a variant glyph is not available
//! for all the characters in the run, simulated glyphs should be
//! synthesized for all characters". So a run never mixes the font's variant
//! glyphs with synthesized ones. Under `PositionSynthesis::SynthesizeMissing`
//! this follows Firefox, which builds the set of glyphs the feature's
//! lookups take (`gfxFontEntry::InputsForOpenTypeFeature`) once per font,
//! script and feature. Firefox then tests each run's nominal glyphs against
//! that set (`gfxFont::SupportsSubSuperscript`). `FeaturesOnly` applies the
//! feature alone and synthesizes nothing. The run loop asks for a coverage the
//! first time a run needs it, and [`FeatureCoverages`] keeps it.
//!
//! **A feature's coverage** is the characters whose nominal glyph the
//! feature substitutes. For a character harfrust fakes where the font does
//! not map it, the nominal glyph is the one it fakes it with. The coverage
//! is a fontwich [`Charset`], tested as a font's coverage is.
//!
//! **Only lookups that substitute a glyph wherever it stands count**: single
//! (GSUB type 1), multiple (2) and alternate (3), and extension lookups (7)
//! wrapping them. Each is read once, in the language system
//! `features::script_features` picks. A ligature (4) substitutes only a
//! sequence, and a contextual lookup (5, 6, 8) only in context, which no set
//! of characters can answer. A glyph only they reach is not covered, which
//! errs toward synthesizing. Firefox counts their input glyphs
//! (`hb_ot_layout_lookup_collect_glyphs`), so it takes a font whose `sups`
//! is written contextually as covering; this does not. Lookup flags are not
//! read, as `collect_glyphs` reads none, so a mark a lookup would skip
//! counts as covered.
//!
//! **Nothing here panics on a font's bytes or takes unbounded time.** An
//! index past its list is passed over, and a table that cannot be read gives
//! nothing. A few kilobytes of offsets can all point at one huge coverage
//! table, so reading stops after [`MAX_WORK`] steps, and what lies past that
//! is not covered.

use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

use fontwich::{Charset, Family, Font, FontKey};
use read_fonts::TableProvider;
use read_fonts::model::Font as HeldFont;
use read_fonts::tables::gsub::{
    AlternateSubst, SingleSubst, SubstitutionLookupList, SubstitutionSubtables,
};
use read_fonts::tables::layout::CoverageTable;

use super::coverage::{FAKED, faked_from};
use super::features;
use super::instance::Instances;
use crate::data::{FxHasher, LruCache, define_id, heap_bytes};

define_id! {
    /// Names what one feature of one font covers under one script, in the
    /// context's table of [`FeatureCoverage`]s.
    struct FeatureCoverageId(u32);
}

/// The most steps one feature's coverage is read in.
///
/// A step is a lookup, a subtable, a coverage glyph, or 64 glyphs of a
/// coverage range. A real font's `sups` takes a few hundred.
const MAX_WORK: u32 = 1 << 20;

/// How many lookup indices there can be, and the fewest glyph ids a feature
/// is read for: sixteen bits each.
const IDS: usize = 1 << 16;

/// The most glyph ids a feature is read for: twenty-four bits, the most a
/// font can have.
const MAX_GLYPHS: u32 = 1 << 24;

/// What `feature` of `font` covers under the OpenType script `script`.
#[derive(Clone, Debug)]
struct FeatureCoverage {
    font: FontKey,
    script: [u8; 4],
    feature: [u8; 4],
    chars: Charset,
}

/// The context's feature coverages, and working memory for making them.
///
/// It is a cache on the context, keyed by font key, script and feature.
/// Each coverage is made the first time a run asks. A change of collection
/// keeps the coverages, since a font's key is its bytes' id. No layout holds
/// a [`FeatureCoverageId`], so each build's start trims the cache, dropping
/// the coverages made or remade longest ago.
pub(super) struct FeatureCoverages {
    table: LruCache<FeatureCoverageId, FeatureCoverage>,
    /// The glyphs the feature being read substitutes, a bit each.
    glyphs: Bits,
    /// The lookups already read for the feature being read, a bit each.
    lookups: Bits,
}

impl FeatureCoverages {
    /// Makes an empty cache that keeps `capacity` coverages, allocating
    /// nothing.
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            table: LruCache::new(capacity),
            glyphs: Bits::new(),
            lookups: Bits::new(),
        }
    }

    /// Sets how many coverages a trim keeps.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.table.set_capacity(capacity);
    }

    /// Drops the coverages used longest ago down to the capacity.
    ///
    /// The walk finds a coverage without counting it as used, so only
    /// [`make`](Self::make) counts a use.
    pub(super) fn trim(&mut self) {
        self.table.trim(|_, _| {});
    }

    /// Drops every coverage, keeping the tables' capacity.
    fn clear(&mut self) {
        self.table.clear();
    }

    /// Returns what `feature` of the font `font` names covers under
    /// `script`.
    ///
    /// Returns `None` where it has not been made. The lookup is by hash; it
    /// reads no font and allocates nothing.
    pub(super) fn find(
        &self,
        font: FontKey,
        script: [u8; 4],
        feature: [u8; 4],
    ) -> Option<&Charset> {
        let hash = key_hash(font, script, feature);
        let found = self.table.peek(hash, |held| {
            held.font == font && held.script == script && held.feature == feature
        })?;
        self.table.get(found).map(|held| &held.chars)
    }

    /// Makes what `feature` of `font`, of `family`, covers under `script`,
    /// if not yet made.
    ///
    /// It reads the font's face in `instances`. Where the face cannot be had
    /// or its `GSUB` read, the coverage is empty, so every font asked about
    /// has an answer and the run loop asks once. Where every id is taken, it
    /// drops every coverage first.
    pub(super) fn make(
        &mut self,
        family: &Family,
        font: &Font,
        instances: &mut Instances,
        script: [u8; 4],
        feature: [u8; 4],
    ) {
        let Some(key) = font.key() else {
            return;
        };
        let hash = key_hash(key, script, feature);
        let found = self.table.find(hash, |held| {
            held.font == key && held.script == script && held.feature == feature
        });
        if found.is_some() {
            return;
        }
        let chars = match instances.face(family, font) {
            Some(face) => {
                self.collect_glyphs(&face.font, script, feature);
                covered(font, &face.font, &self.glyphs)
            }
            None => Charset::default(),
        };
        if self.table.is_full() {
            self.clear();
        }
        let _made = self.table.insert(
            hash,
            FeatureCoverage {
                font: key,
                script,
                feature,
                chars,
            },
        );
    }

    /// Returns how many coverages are made, for tests to see that one is
    /// made once.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.table.len()
    }

    /// Returns the first coverage made, for tests.
    #[cfg(test)]
    pub(super) fn first(&self) -> Option<&Charset> {
        self.table.iter().next().map(|(_, held)| &held.chars)
    }

    /// Sets in `self.glyphs` every glyph that the counted lookups of
    /// `feature` substitute, in `font` under `script`.
    ///
    /// It reads at most [`MAX_WORK`] steps.
    fn collect_glyphs(&mut self, font: &HeldFont, script: [u8; 4], feature: [u8; 4]) {
        let glyphs = font.num_glyphs().clamp(IDS as u32, MAX_GLYPHS);
        self.glyphs.reset(usize::try_from(glyphs).unwrap_or(IDS));
        self.lookups.reset(IDS);
        let Ok(gsub) = font.tables().gsub() else {
            return;
        };
        let Ok(list) = gsub.lookup_list() else {
            return;
        };
        let mut reader = Reader {
            list,
            glyphs: &mut self.glyphs,
            lookups: &mut self.lookups,
            work: 0,
        };
        features::script_features(&gsub, script, |record, list| {
            if record.feature_tag().to_be_bytes() != feature {
                return;
            }
            let Ok(found) = record.feature(list.offset_data()) else {
                return;
            };
            for lookup in found.lookup_list_indices() {
                reader.lookup(lookup.get());
            }
        });
    }
}

heap_bytes! {
    /// Counts its tables and working memory. fontwich's character sets
    /// count at their inline size only.
    FeatureCoverages { table, glyphs, lookups }
}

/// Returns the hash a coverage is found by.
fn key_hash(font: FontKey, script: [u8; 4], feature: [u8; 4]) -> u64 {
    let mut fx = FxHasher::new();
    Hash::hash(&font, &mut fx);
    fx.write(&script);
    fx.write(&feature);
    fx.finish()
}

/// Returns the characters of `font`, held whole as `held`, whose nominal
/// glyph is among `glyphs`.
///
/// The candidates are every character its charset maps, and each one
/// harfrust fakes where the font maps none, through the glyph it fakes it
/// with. The nominal glyph comes from read-fonts' character map on the face,
/// as harfrust maps a character.
fn covered(font: &Font, held: &HeldFont, glyphs: &Bits) -> Charset {
    if glyphs.is_empty() {
        return Charset::default();
    }
    let charmap = held.charmap();
    let charset = font.charset();
    let substituted = |ch: char| {
        charmap
            .map_unicode(ch)
            .is_some_and(|glyph| glyphs.contains(glyph.to_u32()))
    };
    let faked = FAKED
        .iter()
        .copied()
        .filter(|&ch| !charset.contains(ch) && faked_from(ch).is_some_and(substituted));
    charset
        .chars()
        .filter_map(char::from_u32)
        .filter(|&ch| substituted(ch))
        .chain(faked)
        .collect()
}

/// One feature's lookups as they are read, and the work spent.
struct Reader<'a, 'b> {
    list: SubstitutionLookupList<'a>,
    glyphs: &'b mut Bits,
    lookups: &'b mut Bits,
    work: u32,
}

impl Reader<'_, '_> {
    /// Spends `steps` and returns whether the budget still holds.
    fn spend(&mut self, steps: u32) -> bool {
        self.work = self.work.saturating_add(steps);
        self.work <= MAX_WORK
    }

    /// Reads lookup `index` once, where it is a kind that counts.
    fn lookup(&mut self, index: u16) {
        if self.lookups.contains(u32::from(index)) || !self.spend(1) {
            return;
        }
        self.lookups.insert(u32::from(index), u32::from(index));
        let Ok(lookup) = self.list.lookups().get(usize::from(index)) else {
            return;
        };
        // An extension lookup is read through to what it wraps.
        match lookup.subtables() {
            Ok(SubstitutionSubtables::Single(subtables)) => {
                for subtable in subtables.iter() {
                    if !self.spend(1) {
                        return;
                    }
                    let coverage = subtable.and_then(|subtable| match subtable {
                        SingleSubst::Format1(table) => table.coverage(),
                        SingleSubst::Format2(table) => table.coverage(),
                        SingleSubst::Format3(table) => table.coverage(),
                        SingleSubst::Format4(table) => table.coverage(),
                    });
                    self.coverage(coverage.ok());
                }
            }
            Ok(SubstitutionSubtables::Multiple(subtables)) => {
                for subtable in subtables.iter() {
                    if !self.spend(1) {
                        return;
                    }
                    self.coverage(subtable.and_then(|table| table.coverage()).ok());
                }
            }
            Ok(SubstitutionSubtables::Alternate(subtables)) => {
                for subtable in subtables.iter() {
                    if !self.spend(1) {
                        return;
                    }
                    let coverage = subtable.and_then(|subtable| match subtable {
                        AlternateSubst::Format1(table) => table.coverage(),
                        AlternateSubst::Format2(table) => table.coverage(),
                    });
                    self.coverage(coverage.ok());
                }
            }
            // A ligature substitutes a sequence, and a contextual lookup
            // only in context, so neither counts.
            _ => {}
        }
    }

    /// Sets the glyphs `coverage` holds, as far as the work allows.
    ///
    /// Formats 3 and 4 name glyphs in 24 bits, the others in 16.
    fn coverage(&mut self, coverage: Option<CoverageTable<'_>>) {
        match coverage {
            Some(CoverageTable::Format1(table)) => {
                self.listed(table.glyph_array().iter().map(|glyph| glyph.get().to_u32()));
            }
            Some(CoverageTable::Format2(table)) => {
                self.ranges(table.range_records().iter().map(|range| {
                    (
                        range.start_glyph_id().to_u32(),
                        range.end_glyph_id().to_u32(),
                    )
                }));
            }
            Some(CoverageTable::Format3(table)) => {
                self.listed(table.glyph_array().iter().map(|glyph| glyph.get().to_u32()));
            }
            Some(CoverageTable::Format4(table)) => {
                self.ranges(table.range_records().iter().map(|range| {
                    (
                        range.start_glyph_id().to_u32(),
                        range.end_glyph_id().to_u32(),
                    )
                }));
            }
            None => {}
        }
    }

    /// Sets each of `glyphs`, a step each.
    fn listed(&mut self, glyphs: impl Iterator<Item = u32>) {
        for glyph in glyphs {
            if !self.spend(1) {
                return;
            }
            self.glyphs.insert(glyph, glyph);
        }
    }

    /// Sets each range of `ranges`, first to last glyph, a step and one
    /// more for every 64 glyphs. A range that runs backwards is passed over.
    fn ranges(&mut self, ranges: impl Iterator<Item = (u32, u32)>) {
        for (start, end) in ranges {
            if start > end {
                continue;
            }
            if !self.spend(1 + (end - start) / 64) {
                return;
            }
            self.glyphs.insert(start, end);
        }
    }
}

/// A set of ids, a bit each, from zero to the count it was last reset to.
///
/// Its 8 KiB, more for a font of more than 65,536 glyphs, are allocated the
/// first time a coverage is made, and kept.
struct Bits {
    words: Vec<u64>,
    /// Whether any bit is set.
    any: bool,
}

impl Bits {
    const fn new() -> Self {
        Self {
            words: Vec::new(),
            any: false,
        }
    }

    /// Empties the set and sizes it for `ids` ids, allocating its words
    /// where it has too few.
    fn reset(&mut self, ids: usize) {
        self.words.clear();
        self.words.resize(ids.div_ceil(64), 0);
        self.any = false;
    }

    /// Sets `start..=end`, a word at a time. An id past the set's size is
    /// passed over.
    fn insert(&mut self, start: u32, end: u32) {
        let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
            return;
        };
        let end = end.min((self.words.len() * 64).saturating_sub(1));
        if self.words.is_empty() || start > end {
            return;
        }
        let (first, last) = (start / 64, end / 64);
        for (at, word) in self.words.iter_mut().enumerate().take(last + 1).skip(first) {
            let low = if at == first { start % 64 } else { 0 };
            let high = if at == last { end % 64 } else { 63 };
            *word |= (u64::MAX >> (63 - high)) & (u64::MAX << low);
            self.any = true;
        }
    }

    /// Returns whether `id` is set. An id past the set's size never is.
    fn contains(&self, id: u32) -> bool {
        let Ok(id) = usize::try_from(id) else {
            return false;
        };
        self.words
            .get(id / 64)
            .is_some_and(|word| word & (1 << (id % 64)) != 0)
    }

    /// Returns whether no bit is set.
    fn is_empty(&self) -> bool {
        !self.any
    }
}

heap_bytes! {
    Bits { words; any }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use core::iter;
    use read_fonts::model::Blob;

    /// Packs a table: `head`, its 16-bit fields, then `children`.
    ///
    /// Each slot of `slots` pairs a field's index with a child's. That field
    /// holds the child's offset from the table's start.
    fn pack(head: &[u16], slots: &[(usize, usize)], children: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = head.iter().flat_map(|word| word.to_be_bytes()).collect();
        let mut at = Vec::new();
        let mut offset = out.len();
        for child in children {
            at.push(offset);
            offset += child.len();
        }
        for &(slot, child) in slots {
            let offset = u16::try_from(at[child]).expect("a test table under 64 KiB");
            out[2 * slot..2 * slot + 2].copy_from_slice(&offset.to_be_bytes());
        }
        for child in children {
            out.extend_from_slice(child);
        }
        out
    }

    /// A coverage table of `glyphs`, format 1.
    fn glyphs(glyphs: &[u16]) -> Vec<u8> {
        let mut head = vec![1, glyphs.len() as u16];
        head.extend_from_slice(glyphs);
        pack(&head, &[], &[])
    }

    /// A coverage table of `ranges`, format 2.
    fn ranges(ranges: &[(u16, u16)]) -> Vec<u8> {
        let mut head = vec![2, ranges.len() as u16];
        for &(start, end) in ranges {
            head.extend_from_slice(&[start, end, 0]);
        }
        pack(&head, &[], &[])
    }

    /// Returns `value`'s low three bytes, big-endian.
    fn u24(value: u32) -> [u8; 3] {
        let [_, high, middle, low] = value.to_be_bytes();
        [high, middle, low]
    }

    /// A coverage table of `glyphs`, format 3, whose ids are 24-bit.
    fn wide_glyphs(glyphs: &[u32]) -> Vec<u8> {
        let mut out = vec![0, 3];
        out.extend(u24(glyphs.len() as u32));
        out.extend(glyphs.iter().flat_map(|&glyph| u24(glyph)));
        out
    }

    /// A coverage table of `ranges`, format 4, whose ids are 24-bit.
    fn wide_ranges(ranges: &[(u32, u32)]) -> Vec<u8> {
        let mut out = vec![0, 4];
        out.extend(u24(ranges.len() as u32));
        for &(start, end) in ranges {
            out.extend(u24(start).into_iter().chain(u24(end)).chain(u24(0)));
        }
        out
    }

    /// A single substitution subtable, format 3 (a 24-bit delta), over
    /// `coverage`.
    fn wide_single(coverage: Vec<u8>) -> Vec<u8> {
        let mut out = vec![0, 3];
        out.extend(9u32.to_be_bytes());
        out.extend(u24(1));
        out.extend(coverage);
        out
    }

    /// A substitution subtable of GSUB type `kind` over `coverage`, whose
    /// contextual kinds apply lookup 0 to their one glyph.
    fn subtable(kind: u16, coverage: Vec<u8>) -> Vec<u8> {
        match kind {
            // Single (format 1, a delta), multiple, alternate and ligature:
            // the coverage, then nothing this reads.
            1..=4 => pack(&[1, 0, 0], &[(1, 0)], &[coverage]),
            // Contextual, format 3: one glyph of the coverage.
            5 => pack(&[3, 1, 1, 0, 0, 0], &[(3, 0)], &[coverage]),
            // Chained contextual, format 3, with no backtrack or lookahead.
            6 => pack(&[3, 0, 1, 0, 0, 1, 0, 0], &[(3, 0)], &[coverage]),
            _ => unreachable!("a kind the tests build"),
        }
    }

    /// An extension subtable wrapping `inner`, of GSUB type `kind`.
    fn extension(kind: u16, inner: Vec<u8>) -> Vec<u8> {
        pack(&[1, kind, 0, 8], &[], &[inner])
    }

    /// A lookup of GSUB type `kind` with one subtable per item of `slots`,
    /// each naming the one of `children` it points at.
    fn lookup(kind: u16, slots: &[usize], children: &[Vec<u8>]) -> Vec<u8> {
        let mut head = vec![kind, 0, slots.len() as u16];
        head.extend(iter::repeat_n(0, slots.len()));
        let slots: Vec<(usize, usize)> = slots
            .iter()
            .enumerate()
            .map(|(at, &child)| (3 + at, child))
            .collect();
        pack(&head, &slots, children)
    }

    /// Builds a font of nothing but a version 1.0 `GSUB`.
    ///
    /// Its script `DFLT` turns on `sups`, whose lookups are `indices`. The
    /// lookup list has one entry per item of `entries`, each naming the one
    /// of `lookups` it points at. An entry naming none points past the
    /// table.
    fn font(indices: &[u16], entries: &[usize], lookups: &[Vec<u8>]) -> Vec<u8> {
        sfnt(&[(*b"GSUB", gsub(indices, entries, lookups))])
    }

    /// Builds the same font with a `MAXP` of `glyphs` glyphs, a count past
    /// sixteen bits.
    fn wide_font(glyphs: u32, indices: &[u16], entries: &[usize], lookups: &[Vec<u8>]) -> Vec<u8> {
        let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
        maxp.extend(u24(glyphs));
        sfnt(&[
            (*b"GSUB", gsub(indices, entries, lookups)),
            (*b"MAXP", maxp),
        ])
    }

    /// Builds a version 1.0 `GSUB`, as [`font`] describes it.
    fn gsub(indices: &[u16], entries: &[usize], lookups: &[Vec<u8>]) -> Vec<u8> {
        let scripts = pack(&[1, 0x4446, 0x4C54, 8, 4, 0, 0, 0xFFFF, 1, 0], &[], &[]);
        let mut feature = vec![0, indices.len() as u16];
        feature.extend_from_slice(indices);
        let features = pack(
            &[1, 0x7375, 0x7073, 0],
            &[(3, 0)],
            &[pack(&feature, &[], &[])],
        );
        let mut head = vec![entries.len() as u16];
        head.extend(iter::repeat_n(0xFFF0, entries.len()));
        let slots: Vec<(usize, usize)> = entries
            .iter()
            .enumerate()
            .filter(|&(_, &child)| child < lookups.len())
            .map(|(at, &child)| (1 + at, child))
            .collect();
        let list = pack(&head, &slots, lookups);
        pack(
            &[1, 0, 0, 0, 0],
            &[(2, 0), (3, 1), (4, 2)],
            &[scripts, features, list],
        )
    }

    /// Builds a font of `tables`, in tag order, after the directory.
    fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut sfnt = Vec::new();
        for word in [1u16, 0, tables.len() as u16, 0, 0, 0] {
            sfnt.extend_from_slice(&word.to_be_bytes());
        }
        let mut offset = 12 + 16 * tables.len();
        for (tag, table) in tables {
            sfnt.extend_from_slice(tag);
            for word in [0, offset as u32, table.len() as u32] {
                sfnt.extend_from_slice(&word.to_be_bytes());
            }
            offset += table.len();
        }
        for (_, table) in tables {
            sfnt.extend_from_slice(table);
        }
        sfnt
    }

    /// Returns the glyphs `data`'s `sups` substitutes under `DFLT`, as read.
    ///
    /// Returns no glyphs where read-fonts finds no font in it.
    fn substituted(data: &[u8]) -> Vec<u32> {
        let mut coverages = FeatureCoverages::new(usize::MAX);
        if let Some(font) = HeldFont::new(Blob::from(data.to_vec()), 0) {
            coverages.collect_glyphs(&font, *b"DFLT", *b"sups");
        }
        let ids = coverages.glyphs.words.len() as u32 * 64;
        (0..ids)
            .filter(|&glyph| coverages.glyphs.contains(glyph))
            .collect()
    }

    /// Single, multiple and alternate lookups count, directly or through an
    /// extension, each once; a ligature and a contextual lookup do not, nor
    /// does a lookup only a context reaches.
    #[test]
    fn the_lookups_that_substitute_anywhere_count_and_no_others() {
        let lookups = [
            // Reached only through the contextual lookups.
            lookup(1, &[0], &[subtable(1, glyphs(&[30]))]),
            lookup(1, &[0], &[subtable(1, glyphs(&[5]))]),
            lookup(7, &[0], &[extension(1, subtable(1, glyphs(&[6])))]),
            lookup(2, &[0], &[subtable(2, glyphs(&[7]))]),
            lookup(3, &[0], &[subtable(3, ranges(&[(10, 12)]))]),
            lookup(4, &[0], &[subtable(4, glyphs(&[20]))]),
            lookup(5, &[0], &[subtable(5, glyphs(&[21]))]),
            lookup(6, &[0], &[subtable(6, glyphs(&[22]))]),
            lookup(7, &[0], &[extension(5, subtable(5, glyphs(&[23])))]),
        ];
        let entries: Vec<usize> = (0..lookups.len()).collect();
        let data = font(&[1, 2, 3, 4, 5, 6, 7, 8, 1], &entries, &lookups);
        assert_eq!(substituted(&data), [5, 6, 7, 10, 11, 12]);
    }

    /// An index past the lookup list, an offset past the table, a range
    /// that runs backwards and a table cut short read as nothing, and
    /// nothing panics.
    #[test]
    fn broken_lookups_read_as_nothing() {
        let lookups = [
            lookup(1, &[0], &[subtable(1, ranges(&[(9, 3), (4, 4)]))]),
            // Its subtable's coverage lies past its end.
            lookup(1, &[0], &[pack(&[1, 0xFFF0, 0], &[], &[])]),
        ];
        // Entry 2 points past the table; indices 3 and 0xFFFF past the list.
        let data = font(&[2, 3, u16::MAX, 1, 0], &[0, 1, 9], &lookups);
        assert_eq!(substituted(&data), [4]);
        for cut in [0, 12, 30, data.len() / 2, data.len() - 1] {
            let _ = substituted(&data[..cut]);
        }
    }

    /// A `GSUB` that asks for endless work is read within the budget.
    ///
    /// Thousands of lookup entries point at one lookup of thousands of
    /// subtables, which all point at one coverage of thousands of ranges,
    /// each over every glyph. What the read reached is covered, and the read
    /// ends.
    #[test]
    fn a_gsub_asking_for_endless_work_is_read_within_the_budget() {
        let coverage = ranges(&[(0, u16::MAX); 3000]);
        let single = lookup(1, &[0; 4000], &[subtable(1, coverage)]);
        let indices: Vec<u16> = (0..4000).collect();
        let data = font(&indices, &[0; 4000], &[single]);
        assert_eq!(substituted(&data).len(), IDS);
    }

    /// A font of more than 65,536 glyphs is read past sixteen bits: the
    /// 24-bit coverages, format 3 and 4, in a 24-bit single substitution,
    /// up to its glyph count and no further.
    #[test]
    fn coverages_past_sixteen_bits_are_read_up_to_the_glyph_count() {
        let lookups = [
            lookup(1, &[0], &[wide_single(wide_glyphs(&[3, 70_000, 90_000]))]),
            lookup(1, &[0], &[wide_single(wide_ranges(&[(65_530, 65_540)]))]),
            lookup(1, &[0], &[subtable(1, glyphs(&[9]))]),
        ];
        let data = wide_font(70_002, &[0, 1, 2], &[0, 1, 2], &lookups);
        let expected: Vec<u32> = [3, 9]
            .into_iter()
            .chain(65_530..=65_540)
            .chain([70_000])
            .collect();
        assert_eq!(substituted(&data), expected);
        // Without a count past sixteen bits, only those ids are read.
        let data = font(&[0, 1, 2], &[0, 1, 2], &lookups);
        assert_eq!(
            substituted(&data),
            [3, 9, 65_530, 65_531, 65_532, 65_533, 65_534, 65_535]
        );
    }

    #[test]
    fn bits_set_ranges_across_words_and_nothing_else() {
        let mut bits = Bits::new();
        assert!(!bits.contains(0), "empty before its first reset");
        bits.insert(0, 5);
        assert!(!bits.contains(0), "nothing set before its first reset");
        bits.reset(IDS);
        assert!(bits.is_empty());
        bits.insert(60, 130);
        bits.insert(7, 7);
        bits.insert(65_535, 70_000);
        bits.insert(80_000, 80_000);
        let set: Vec<u32> = (0..IDS as u32 + 2)
            .filter(|&id| bits.contains(id))
            .collect();
        let expected: Vec<u32> = [7].into_iter().chain(60..=130).chain([65_535]).collect();
        assert_eq!(set, expected);
        assert!(!bits.is_empty());
        bits.reset(IDS + 1);
        assert!(bits.is_empty() && !bits.contains(7));
        bits.insert(65_536, 70_000);
        assert!(bits.contains(65_536) && !bits.contains(65_600));
    }

    /// `FAKED` lists every character harfrust fakes, and nothing else.
    #[test]
    fn the_faked_characters_are_the_ones_faked() {
        let faked: Vec<char> = (0..0x3100)
            .filter_map(char::from_u32)
            .filter(|&ch| faked_from(ch).is_some())
            .collect();
        let mut listed = FAKED.to_vec();
        listed.sort_unstable();
        assert_eq!(faked, listed);
    }
}
