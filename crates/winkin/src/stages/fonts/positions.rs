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
//! that set (`gfxFont::SupportsSubSuperscript`). Chrome applies the feature
//! alone and synthesizes nothing (crbug.com/352218916), as `FeaturesOnly`
//! does. The run loop asks for a coverage the first time a run needs it, and
//! [`FeatureCoverages`] keeps it.
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
use read_fonts::tables::gsub::{SingleSubst, SubstitutionLookupList, SubstitutionSubtables};
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

/// How many glyph ids, and lookup indices, there can be: sixteen bits each.
const IDS: usize = 1 << 16;

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
        self.glyphs.reset();
        self.lookups.reset();
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
        self.lookups.insert(index, index);
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
                    self.coverage(subtable.and_then(|table| table.coverage()).ok());
                }
            }
            // A ligature substitutes a sequence, and a contextual lookup
            // only in context, so neither counts.
            _ => {}
        }
    }

    /// Sets the glyphs `coverage` holds, as far as the work allows.
    fn coverage(&mut self, coverage: Option<CoverageTable<'_>>) {
        match coverage {
            Some(CoverageTable::Format1(table)) => {
                for glyph in table.glyph_array() {
                    if !self.spend(1) {
                        return;
                    }
                    let glyph = glyph.get().to_u16();
                    self.glyphs.insert(glyph, glyph);
                }
            }
            Some(CoverageTable::Format2(table)) => {
                for range in table.range_records() {
                    let (start, end) = (
                        range.start_glyph_id().to_u16(),
                        range.end_glyph_id().to_u16(),
                    );
                    if start > end {
                        continue;
                    }
                    let words = u32::from(end - start) / 64;
                    if !self.spend(1 + words) {
                        return;
                    }
                    self.glyphs.insert(start, end);
                }
            }
            None => {}
        }
    }
}

/// A set of sixteen-bit ids, a bit each.
///
/// Its 8 KiB are allocated the first time a coverage is made, and kept.
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

    /// Empties the set, allocating its words the first time.
    fn reset(&mut self) {
        self.words.clear();
        self.words.resize(IDS / 64, 0);
        self.any = false;
    }

    /// Sets `start..=end`, a word at a time.
    fn insert(&mut self, start: u16, end: u16) {
        let (start, end) = (usize::from(start), usize::from(end));
        let (first, last) = (start / 64, end / 64);
        for (at, word) in self.words.iter_mut().enumerate().take(last + 1).skip(first) {
            let low = if at == first { start % 64 } else { 0 };
            let high = if at == last { end % 64 } else { 63 };
            *word |= (u64::MAX >> (63 - high)) & (u64::MAX << low);
            self.any = true;
        }
    }

    /// Returns whether `id` is set. An id past sixteen bits never is.
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
        let gsub = pack(
            &[1, 0, 0, 0, 0],
            &[(2, 0), (3, 1), (4, 2)],
            &[scripts, features, list],
        );
        // One table, `GSUB`, after the directory.
        let mut sfnt = Vec::new();
        for word in [1u16, 0, 1, 0, 0, 0] {
            sfnt.extend_from_slice(&word.to_be_bytes());
        }
        sfnt.extend_from_slice(b"GSUB");
        for word in [0u32, 28, gsub.len() as u32] {
            sfnt.extend_from_slice(&word.to_be_bytes());
        }
        sfnt.extend_from_slice(&gsub);
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
        (0..IDS as u32)
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

    #[test]
    fn bits_set_ranges_across_words_and_nothing_else() {
        let mut bits = Bits::new();
        assert!(!bits.contains(0), "empty before its first reset");
        bits.reset();
        assert!(bits.is_empty());
        bits.insert(60, 130);
        bits.insert(7, 7);
        bits.insert(u16::MAX, u16::MAX);
        let set: Vec<u32> = (0..IDS as u32 + 2)
            .filter(|&id| bits.contains(id))
            .collect();
        let expected: Vec<u32> = [7]
            .into_iter()
            .chain(60..=130)
            .chain([u32::from(u16::MAX)])
            .collect();
        assert_eq!(set, expected);
        assert!(!bits.is_empty());
        bits.reset();
        assert!(bits.is_empty() && !bits.contains(7));
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
