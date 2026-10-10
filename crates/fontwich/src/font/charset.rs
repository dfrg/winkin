//! Which characters a font maps: its cmap, as a bitmap.
//!
//! Only Unicode subtables count. A symbol subtable (Windows encoding 0) maps
//! Wingdings' pictures to U+F020–F0FF, and a Macintosh one maps Mac Roman
//! codes; taking either as Unicode coverage would have a dingbat font claim
//! the private use area and a Mac font claim Latin-1 by accident. read-fonts'
//! `best_subtable` prefers the symbol subtable, which is right for shaping a
//! symbol font and wrong for asking what it covers, so the choice is made here.
//!
//! Format 13 does not count either. It maps whole ranges to one glyph, which is
//! how a last-resort font draws a placeholder for every character in a block;
//! that is not covering them.
//!
//! # Why a bitmap, and why shared
//!
//! Measured over every installed font on Windows 11 and Fedora 44
//! (`tests/heap.rs`), sorted ranges cost 760 KB and 3.6 MB of heap. CJK fonts
//! are fragmented enough that 256-character pages of bits come out smaller,
//! and they make union and difference word operations, which is what asking
//! whether a family list covers a set of characters wants. Then most fonts map
//! exactly what a sibling does — every weight of a family, usually — so a
//! layer keeps one copy of each distinct charset: 147 of 391 fonts on Windows,
//! 236 of 2433 on Fedora, and under 200 KB either way.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::ops::RangeInclusive;

use read_fonts::tables::cmap::{Cmap, CmapSubtable};
use read_fonts::{FontRef, TableProvider};

use crate::sort;

/// A set of characters mapped by a font.
///
/// Stored as a sparse bitmap of 256-character pages. An empty set requires
/// no allocations; a nonempty set uses two.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Charset {
    /// Which pages it has, 64 to a row, from page 0 to its last: a
    /// character's page is its value over 256.
    index: Box<[PageRow]>,
    /// Each page's 256 bits, in page order.
    leaves: Box<[[u64; 4]]>,
}

/// 64 consecutive pages of a [`Charset`]'s index: which of them it maps
/// anything on, and how many pages it has before them, which is where the
/// first of their leaves is.
///
/// So finding a page's leaf is a load, a bit test and a count of the bits
/// below it, where a list of page numbers would be searched.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
struct PageRow {
    /// Bit `n` for page `64 * row + n`.
    present: u64,
    /// How many pages the rows before this one have.
    before: u32,
}

impl Charset {
    /// Returns the number of characters.
    pub fn count(&self) -> usize {
        self.leaves
            .iter()
            .flatten()
            .map(|bits| bits.count_ones() as usize)
            .sum()
    }

    /// Returns `true` if the set contains no characters.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Returns `true` if the set contains `c`.
    pub fn contains(&self, c: char) -> bool {
        self.contains_u32(u32::from(c))
    }

    /// Returns `true` if the set contains every character.
    ///
    /// Returns `true` for an empty iterator. Stops at the first missing
    /// character and reuses page lookups for adjacent characters on the
    /// same page. Does not allocate.
    pub fn covers_all(&self, chars: impl IntoIterator<Item = char>) -> bool {
        let mut held: Option<CharsetPage<'_>> = None;
        chars.into_iter().all(|c| {
            let page = match held {
                Some(page) if page.holds(c) => page,
                _ => match self.page(c) {
                    Some(page) => *held.insert(page),
                    None => return false,
                },
            };
            page.contains(c)
        })
    }

    /// Returns the page containing `c`, if nonempty.
    ///
    /// Each page spans 256 code points. Retain the result when checking
    /// multiple characters on the same page to avoid repeated lookups. See
    /// [`CharsetPage::holds`] and [`CharsetPage::contains`].
    pub fn page(&self, c: char) -> Option<CharsetPage<'_>> {
        let number = u32::from(c) >> 8;
        Some(CharsetPage {
            number,
            bits: self.leaf(number)?,
        })
    }

    /// Returns character code points in ascending order.
    pub fn chars(&self) -> impl Iterator<Item = u32> + '_ {
        self.leaves().flat_map(|(page, leaf)| {
            leaf.iter().enumerate().flat_map(move |(word, &bits)| {
                let base = (page << 8) | ((word as u32) << 6);
                ones(bits).map(move |bit| base | bit)
            })
        })
    }

    /// Returns consecutive character ranges in ascending order.
    pub fn ranges(&self) -> impl Iterator<Item = RangeInclusive<u32>> + '_ {
        let mut chars = self.chars().peekable();
        core::iter::from_fn(move || {
            let start = chars.next()?;
            let mut end = start;
            while chars.next_if_eq(&(end + 1)).is_some() {
                end += 1;
            }
            Some(start..=end)
        })
    }

    /// Whether it maps the character with value `c`.
    pub(super) fn contains_u32(&self, c: u32) -> bool {
        self.leaf(c >> 8).is_some_and(|leaf| has(leaf, c))
    }

    /// What its two boxes hold.
    pub(crate) fn heap(&self) -> usize {
        core::mem::size_of_val(&*self.index) + core::mem::size_of_val(&*self.leaves)
    }

    /// A hash of what it maps, for a layer to find an identical charset
    /// among those it holds without comparing it against every one.
    pub(crate) fn hash(&self) -> u64 {
        use core::hash::{BuildHasher, Hasher};
        let mut hasher = crate::hash::Build.build_hasher();
        for row in &self.index {
            hasher.write_u64(row.present);
        }
        for word in self.leaves.iter().flatten() {
            hasher.write_u64(*word);
        }
        hasher.finish()
    }

    /// Its bits for the 256 characters on page `page`, or `None` where it
    /// maps none of them: the word operations [`CharsetPage`] keeps to
    /// itself.
    ///
    /// The page's row of the index says whether it is there, and how many
    /// pages come before it: those of the rows before, and those of its own
    /// row below it.
    fn leaf(&self, page: u32) -> Option<&[u64; 4]> {
        let row = self.index.get((page >> 6) as usize)?;
        let bit = 1u64 << (page & 63);
        if row.present & bit == 0 {
            return None;
        }
        let at = row.before as usize + (row.present & (bit - 1)).count_ones() as usize;
        self.leaves.get(at)
    }

    /// What both it and `other` map.
    pub(super) fn intersection(&self, other: &Charset) -> Charset {
        let mut builder = Builder::default();
        for (page, leaf) in self.leaves() {
            if let Some(bits) = other.leaf(page) {
                let both = [
                    leaf[0] & bits[0],
                    leaf[1] & bits[1],
                    leaf[2] & bits[2],
                    leaf[3] & bits[3],
                ];
                if both != [0; 4] {
                    builder.pages.push((page, both));
                }
            }
        }
        builder.finish()
    }

    /// Every page it maps anything on, with its bits there, in order.
    fn leaves(&self) -> impl Iterator<Item = (u32, &[u64; 4])> {
        let pages = self
            .index
            .iter()
            .zip(0u32..)
            .flat_map(|(row, at)| ones(row.present).map(move |bit| at << 6 | bit));
        pages.zip(self.leaves.iter())
    }

    /// The charset of `font`. Empty when it has no Unicode subtable.
    pub(super) fn from_font(font: &FontRef<'_>) -> Self {
        font.cmap()
            .ok()
            .and_then(|cmap| best(&cmap))
            .map(Self::from_subtable)
            .unwrap_or_default()
    }

    fn from_subtable(subtable: CmapSubtable<'_>) -> Self {
        let mut builder = Builder::default();
        match subtable {
            CmapSubtable::Format12(table) => {
                for group in table.groups() {
                    let (mut start, end) = (group.start_char_code(), group.end_char_code());
                    // The group's first character takes its first glyph, and
                    // glyph 0 is .notdef, which is not a mapping.
                    if group.start_glyph_id() == 0 {
                        start = start.saturating_add(1);
                    }
                    builder.insert(start, end.min(MAX));
                }
            }
            CmapSubtable::Format4(table) => format4(&table, &mut builder),
            // The rest are at most 64K characters, few enough to walk.
            other => {
                for (c, glyph) in other.iter() {
                    if glyph.to_u32() != 0 && c <= MAX {
                        builder.insert(c, c);
                    }
                }
            }
        }
        builder.finish()
    }
}

impl FromIterator<char> for Charset {
    /// Creates a set from characters.
    ///
    /// Input order and duplicates do not affect the result. Construction
    /// allocates temporary storage in addition to the finished set.
    fn from_iter<I: IntoIterator<Item = char>>(chars: I) -> Self {
        let mut builder = Builder::default();
        for c in chars {
            let c = u32::from(c);
            builder.insert(c, c);
        }
        builder.finish()
    }
}

/// The positions of the bits set in `bits`, lowest first.
fn ones(mut bits: u64) -> impl Iterator<Item = u32> {
    core::iter::from_fn(move || {
        if bits == 0 {
            return None;
        }
        let bit = bits.trailing_zeros();
        bits &= bits - 1;
        Some(bit)
    })
}

/// Whether `leaf`, the bits of the page `c` is on, has `c`'s bit set.
///
/// A character is on page `c >> 8`, and there it is bit `c & 63` of word
/// `(c & 0xFF) >> 6`, lowest bit first.
fn has(leaf: &[u64; 4], c: u32) -> bool {
    leaf[(c as usize & 0xFF) >> 6] & (1 << (c & 63)) != 0
}

/// A nonempty 256-character page of a [`Charset`].
///
/// A borrowed view for repeated coverage checks.
/// [`contains`](Self::contains) returns `false` for characters outside this
/// page, even if the charset contains them. Use [`holds`](Self::holds) to
/// check whether a new page lookup is needed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CharsetPage<'a> {
    /// Which page: its characters' values over 256.
    number: u32,
    bits: &'a [u64; 4],
}

impl CharsetPage<'_> {
    /// Returns `true` if `c` is within this page.
    pub fn holds(self, c: char) -> bool {
        u32::from(c) >> 8 == self.number
    }

    /// Returns `true` if this page contains `c`.
    pub fn contains(self, c: char) -> bool {
        self.holds(c) && has(self.bits, u32::from(c))
    }
}

/// A format 4 subtable, segment by segment rather than character by
/// character: the mapping is read-fonts', but most segments map by a delta,
/// and those go in whole.
fn format4(table: &read_fonts::tables::cmap::Cmap4<'_>, builder: &mut Builder) {
    let (starts, ends) = (table.start_code(), table.end_code());
    let (deltas, offsets) = (table.id_delta(), table.id_range_offsets());
    let glyphs = table.glyph_id_array();
    let segments = starts
        .len()
        .min(ends.len())
        .min(deltas.len())
        .min(offsets.len());
    for at in 0..segments {
        let (start, end) = (u32::from(starts[at].get()), u32::from(ends[at].get()));
        if start > end {
            continue;
        }
        let delta = deltas[at].get() as i32;
        let offset = usize::from(offsets[at].get());
        // Glyph 0 is .notdef, which is not a mapping.
        let maps = |glyph: i32| (glyph + delta) as u16 != 0;
        if offset == 0 {
            // `c + delta` wraps to 0 for one character at most.
            let zero = (-delta) as u16 as u32;
            if (start..=end).contains(&zero) {
                if zero > start {
                    builder.insert(start, zero - 1);
                }
                if zero < end {
                    builder.insert(zero + 1, end);
                }
            } else {
                builder.insert(start, end);
            }
            continue;
        }
        // Through the glyph array, where 0 means unmapped before the delta
        // is added: runs of mapped characters go in together.
        let mut run: Option<u32> = None;
        for c in start..=end {
            // Saturating, as read-fonts does, so a broken offset reads what
            // its lookup would.
            let index = (offset / 2 + (c - start) as usize).saturating_sub(offsets.len() - at);
            let glyph = glyphs.get(index).map_or(0, |glyph| glyph.get());
            let mapped = glyph != 0 && maps(i32::from(glyph));
            match (mapped, run) {
                (true, None) => run = Some(c),
                (false, Some(from)) => {
                    builder.insert(from, c - 1);
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(from) = run {
            builder.insert(from, end);
        }
    }
}

/// Pages as they are filled. A cmap lists characters in order, so nearly
/// every insert lands on the last page or starts the next; one out of order
/// starts a duplicate, which `finish` folds in.
#[derive(Default)]
pub(super) struct Builder {
    pages: Vec<(u32, [u64; 4])>,
}

impl Builder {
    /// Sets `start..=end`, which may be empty.
    pub(crate) fn insert(&mut self, start: u32, end: u32) {
        let mut c = start;
        while c <= end {
            let page = c >> 8;
            let last = end.min(page << 8 | 0xFF);
            let leaf = match self.pages.last_mut() {
                Some((at, leaf)) if *at == page => leaf,
                _ => {
                    self.pages.push((page, [0; 4]));
                    &mut self.pages.last_mut().expect("just pushed").1
                }
            };
            let (first_word, last_word) = ((c & 0xFF) >> 6, (last & 0xFF) >> 6);
            for word in first_word..=last_word {
                let low = if word == first_word { c & 63 } else { 0 };
                let high = if word == last_word { last & 63 } else { 63 };
                leaf[word as usize] |= (u64::MAX >> (63 - high)) & (u64::MAX << low);
            }
            if last == MAX {
                break;
            }
            c = last + 1;
        }
    }

    pub(crate) fn finish(mut self) -> Charset {
        // Equal pages come out adjacent, to be merged: in any order, as
        // merging joins their bits.
        sort::by_key(&mut self.pages, |&(page, _)| page);
        let mut pages: Vec<u32> = Vec::with_capacity(self.pages.len());
        let mut leaves: Vec<[u64; 4]> = Vec::with_capacity(self.pages.len());
        for (page, leaf) in self.pages {
            if pages.last() == Some(&page) {
                let last = leaves.last_mut().expect("one leaf per page");
                for (into, from) in last.iter_mut().zip(leaf) {
                    *into |= from;
                }
            } else {
                pages.push(page);
                leaves.push(leaf);
            }
        }
        let Some(&last) = pages.last() else {
            return Charset::default();
        };
        // A row for every 64 pages up to the last, each counting the pages
        // before it.
        let mut index = alloc::vec![PageRow::default(); (last >> 6) as usize + 1];
        for &page in &pages {
            if let Some(row) = index.get_mut((page >> 6) as usize) {
                row.present |= 1 << (page & 63);
            }
        }
        let mut before = 0;
        for row in &mut index {
            row.before = before;
            before += row.present.count_ones();
        }
        Charset {
            index: index.into_boxed_slice(),
            leaves: leaves.into_boxed_slice(),
        }
    }
}

impl fmt::Debug for Charset {
    // Hundreds of pages for a CJK font, which would drown any `Font`
    // printed with one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Charset")
            .field("chars", &self.count())
            .field("pages", &self.leaves.len())
            .finish()
    }
}

/// The largest Unicode scalar value.
const MAX: u32 = 0x10FFFF;

/// How good a subtable is for coverage, best first; `None` if it does not
/// count. Full-repertoire subtables beat BMP ones, which beat the old Unicode
/// platform encodings.
fn rank(platform: u16, encoding: u16) -> Option<u8> {
    match (platform, encoding) {
        (3, 10) => Some(0),
        (0, 4) => Some(1),
        (3, 1) => Some(2),
        (0, 3) => Some(3),
        (0, 0..=2) => Some(4),
        _ => None,
    }
}

/// The subtable [`rank`] likes best, among those it can read.
fn best<'a>(cmap: &Cmap<'a>) -> Option<CmapSubtable<'a>> {
    cmap.encoding_records()
        .iter()
        .filter_map(|record| {
            let rank = rank(record.platform_id() as u16, record.encoding_id())?;
            let subtable = record.subtable(cmap.offset_data()).ok()?;
            usable(&subtable).then_some((rank, subtable))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, subtable)| subtable)
}

fn usable(subtable: &CmapSubtable<'_>) -> bool {
    matches!(
        subtable,
        CmapSubtable::Format4(_)
            | CmapSubtable::Format6(_)
            | CmapSubtable::Format10(_)
            | CmapSubtable::Format12(_)
    )
}

/// What the `cmap` of a font on disk says, reading its encoding records and
/// only its chosen subtable: its charset, empty when it has no Unicode
/// subtable or it cannot be read; and where its variation sequences are,
/// where a record points to them.
#[cfg(feature = "std")]
pub(crate) fn from_file(
    font: &mut super::sfnt::FileFont,
) -> (Charset, Option<super::sequences::Sequences>) {
    read_file(font).unwrap_or_default()
}

#[cfg(feature = "std")]
fn read_file(
    font: &mut super::sfnt::FileFont,
) -> Option<(Charset, Option<super::sequences::Sequences>)> {
    use super::sequences::Sequences;
    use super::sfnt::{TABLE_LIMIT, read_u16, read_u32};
    use read_fonts::{FontData, FontRead};

    let Some((cmap, length)) = font.locate(b"cmap") else {
        return Some((Charset::default(), None));
    };
    let at = u64::from(cmap);
    let header = font.read(at, 4)?;
    let records = font.read(at + 4, 8 * u32::from(read_u16(&header, 2)?))?;

    // The encoding records, then just the subtable they point to: a cmap
    // also holds format 14 variation sequences, which for a CJK font can be
    // larger than the mapping itself. The candidates are tried best first,
    // since the best may be a format this does not read.
    let mut offsets: Vec<(u32, Option<u8>)> = records
        .as_chunks::<8>()
        .0
        .iter()
        .filter_map(|record| {
            let rank = rank(read_u16(record, 0)?, read_u16(record, 2)?);
            Some((read_u32(record, 4)?, rank))
        })
        .collect();
    // The variation sequences are where the Unicode Variation Sequences
    // record says, platform 0 encoding 5, which is where the format 14
    // subtable is kept; they are noted, not read (see `sequences`). A
    // subtable of another format there reads as no sequences when asked.
    let sequences = records
        .as_chunks::<8>()
        .0
        .iter()
        .find(|record| {
            read_u16(record.as_slice(), 0) == Some(0) && read_u16(record.as_slice(), 2) == Some(5)
        })
        .and_then(|record| read_u32(record.as_slice(), 4))
        .filter(|&offset| offset < length)
        .and_then(|offset| Some(Sequences::new(cmap.checked_add(offset)?, length - offset)));
    // A subtable ends where the next begins, or with the table. Its own
    // length field is no help: format 4's is sixteen bits, and a CJK font's
    // format 4 can be longer than that says.
    sort::by(&mut offsets, Ord::cmp);
    let extent = |offset: u32| {
        let next = offsets
            .iter()
            .map(|&(other, _)| other)
            .find(|&other| other > offset)
            .unwrap_or(length);
        next.min(length).saturating_sub(offset).min(TABLE_LIMIT)
    };
    let mut candidates: Vec<(u8, u32)> = offsets
        .iter()
        .filter_map(|&(offset, rank)| Some((rank?, offset)))
        .collect();
    sort::by(&mut candidates, Ord::cmp);
    for (_, offset) in candidates {
        let Some(bytes) = font.read(at + u64::from(offset), extent(offset)) else {
            continue;
        };
        if let Ok(subtable) = CmapSubtable::read(FontData::new(&bytes))
            && usable(&subtable)
        {
            return Some((Charset::from_subtable(subtable), sequences));
        }
    }
    Some((Charset::default(), sequences))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{cmap, font_with_tables, format4, format4_segments, format12};
    use alloc::vec;

    fn charset(font: &[u8]) -> Charset {
        Charset::from_font(&FontRef::new(font).expect("a font"))
    }

    fn font(subtables: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        font_with_tables(&[(*b"cmap", cmap(subtables))])
    }

    #[test]
    fn format4_segments_map_as_read_fonts_reads_them() {
        let table = format4_segments(&[
            // By delta, where U+0045 comes to glyph 0: 0x45 - 0x45.
            (0x41, 0x4A, -0x45, &[]),
            // Through the array, with holes; the last glyph wraps to 0 with
            // the delta, so it is not a mapping either.
            (0x61, 0x66, 1, &[5, 0, 7, 0, 9, 0xFFFF]),
        ]);
        let bytes = font(&[(3, 1, table)]);
        let font = FontRef::new(&bytes).expect("a font");
        let subtable = best(&font.cmap().expect("a cmap")).expect("a subtable");
        let walked: Vec<u32> = subtable
            .iter()
            .filter(|(_, glyph)| glyph.to_u32() != 0)
            .map(|(c, _)| c)
            .collect();
        let ours: Vec<u32> = charset(&bytes).chars().collect();
        assert_eq!(ours, walked);
        assert_eq!(
            ours,
            [
                0x41, 0x42, 0x43, 0x44, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x61, 0x63, 0x65
            ]
        );
    }

    #[test]
    fn a_format_4_font_covers_what_it_maps() {
        let map = charset(&font(&[(3, 1, format4(&[(0x20, 0x7E), (0xA0, 0xFF)]))]));
        assert!(map.contains('A') && map.contains('ÿ'));
        assert!(!map.contains('\u{7F}') && !map.contains('Ā'));
        assert_eq!(map.ranges().collect::<Vec<_>>(), [0x20..=0x7E, 0xA0..=0xFF]);
        assert_eq!(map.count(), 95 + 96);
    }

    #[test]
    fn adjacent_ranges_merge() {
        let map = charset(&font(&[(3, 1, format4(&[(0x41, 0x4F), (0x50, 0x5A)]))]));
        assert_eq!(map.ranges().collect::<Vec<_>>(), vec![0x41..=0x5A]);
    }

    #[test]
    fn ranges_across_page_and_word_boundaries_round_trip() {
        let mut builder = Builder::default();
        for (start, end) in [(0x3F, 0x41), (0xFE, 0x102), (0x1F0, 0x2FF), (0x10FFFE, MAX)] {
            builder.insert(start, end);
        }
        // Out of order, and overlapping what is there.
        builder.insert(0x100, 0x100);
        builder.insert(0x40, 0x7F);
        let map = builder.finish();
        assert_eq!(
            map.ranges().collect::<Vec<_>>(),
            vec![0x3F..=0x7F, 0xFE..=0x102, 0x1F0..=0x2FF, 0x10FFFE..=MAX]
        );
        assert_eq!(map.count(), 65 + 5 + 272 + 2);
        assert!(map.contains('\u{10FFFF}') && !map.contains('\u{80}'));
    }

    #[test]
    fn the_rank_index_finds_every_page_across_its_rows() {
        // Either side of each row's edge, and the last page there is.
        let pages = [0, 1, 63, 64, 65, 127, 128, 0x1F6, MAX >> 8];
        let mut builder = Builder::default();
        for &page in pages.iter().rev() {
            // A different character on each page, so each leaf is its own.
            let c = page << 8 | (page & 0xFF);
            builder.insert(c, c);
        }
        let map = builder.finish();
        let found: Vec<u32> = map.leaves().map(|(page, _)| page).collect();
        assert_eq!(found, pages);
        for page in 0..(MAX >> 8) + 70 {
            let expected = pages.contains(&page).then(|| {
                let mut leaf = [0u64; 4];
                leaf[(page as usize & 0xFF) >> 6] = 1 << (page & 63);
                leaf
            });
            assert_eq!(map.leaf(page).copied(), expected, "page {page:#x}");
        }
    }

    #[test]
    fn identical_charsets_hash_alike_and_different_ones_do_not() {
        let build = |ranges: &[(u32, u32)]| {
            let mut builder = Builder::default();
            for &(start, end) in ranges {
                builder.insert(start, end);
            }
            builder.finish()
        };
        let a = build(&[(0x20, 0x7E)]);
        let b = build(&[(0x20, 0x4F), (0x50, 0x7E)]);
        let c = build(&[(0x20, 0x7D)]);
        assert_eq!(a, b);
        assert_eq!(a.hash(), b.hash());
        assert_ne!(a.hash(), c.hash());
        assert_eq!(Charset::default(), build(&[]));
    }

    /// Characters collected in any order, repeated, across pages and words,
    /// make the charset the builder makes of their ranges, and none make the
    /// empty one.
    #[test]
    fn a_charset_collected_from_characters_is_the_one_built_from_their_ranges() {
        let ranges = [
            (0x30, 0x39),
            (0xFE, 0x102),
            (0x4E00, 0x4E3F),
            (0x1F600, 0x1F600),
        ];
        let mut builder = Builder::default();
        for (start, end) in ranges {
            builder.insert(start, end);
        }
        let built = builder.finish();
        let mut chars: Vec<char> = ranges
            .iter()
            .flat_map(|&(start, end)| start..=end)
            .filter_map(char::from_u32)
            .collect();
        chars.reverse();
        chars.extend(['5', '\u{4E10}', '\u{1F600}']);
        let collected: Charset = chars.into_iter().collect();
        assert_eq!(collected, built);
        assert_eq!(collected.hash(), built.hash());
        assert!(collected.page('7').is_some_and(|page| page.contains('7')));
        assert!(!collected.contains('a') && !collected.contains('\u{103}'));
        agrees(&collected, usize::MAX);
        assert_eq!(
            core::iter::empty::<char>().collect::<Charset>(),
            Charset::default()
        );
    }

    #[test]
    fn a_full_repertoire_subtable_wins_over_a_bmp_one() {
        let map = charset(&font(&[
            (3, 1, format4(&[(0x41, 0x41)])),
            (3, 10, format12(&[(0x41, 0x41), (0x1F600, 0x1F64F)])),
        ]));
        assert!(map.contains('A') && map.contains('😀'));
    }

    #[test]
    fn a_symbol_subtable_covers_nothing() {
        // Wingdings' shape: everything in U+F020–F0FF, under encoding 0.
        let map = charset(&font(&[(3, 0, format4(&[(0xF020, 0xF0FF)]))]));
        assert!(map.is_empty());
    }

    #[test]
    fn a_symbol_subtable_beside_a_unicode_one_is_ignored() {
        let map = charset(&font(&[
            (3, 0, format4(&[(0xF020, 0xF0FF)])),
            (3, 1, format4(&[(0x41, 0x5A)])),
        ]));
        assert!(map.contains('A') && !map.contains('\u{F041}'));
    }

    #[test]
    fn a_font_with_no_cmap_covers_nothing() {
        let font = font_with_tables(&[(*b"head", alloc::vec![0; 54])]);
        assert!(charset(&font).is_empty());
    }

    #[test]
    fn a_group_starting_at_notdef_does_not_cover_its_first_character() {
        let mut table = format12(&[(0x40, 0x42)]);
        // The one group's startGlyphID, the last four bytes.
        let at = table.len() - 4;
        table[at..].copy_from_slice(&0u32.to_be_bytes());
        let map = charset(&font(&[(3, 10, table)]));
        assert!(!map.contains('@') && map.contains('A') && map.contains('B'));
    }

    #[test]
    fn a_group_past_unicode_is_cut_to_it() {
        let map = charset(&font(&[(3, 10, format12(&[(0x10FFF0, 0xFFFFFFFF)]))]));
        assert_eq!(map.count(), 16);
    }

    /// Checks [`Charset::page`] and [`Charset::covers_all`] against
    /// [`Charset::contains`]: every character of up to `most` of the pages
    /// `map` has, and of the pages either side of them, one by one and as
    /// clusters of every length to eight, on one page, across two, and
    /// going back and forth between them. A page is asked about its own
    /// characters found fresh and kept from the first, and about the
    /// characters of the page after it, which it must not claim.
    fn agrees(map: &Charset, most: usize) {
        let has: Vec<u32> = map.leaves().map(|(page, _)| page).collect();
        let step = has.len().div_ceil(most.max(1)).max(1);
        let mut pages: Vec<u32> = has
            .iter()
            .step_by(step)
            .flat_map(|&page| [page.saturating_sub(1), page, (page + 1).min(MAX >> 8)])
            .collect();
        pages.sort_unstable();
        pages.dedup();
        let mut chars: Vec<char> = Vec::new();
        let mut before: Option<CharsetPage<'_>> = None;
        for page in pages {
            let mut kept: Option<Option<CharsetPage<'_>>> = None;
            for c in (page << 8..=(page << 8 | 0xFF)).filter_map(char::from_u32) {
                let v = u32::from(c);
                let fresh = map.page(c);
                let kept = *kept.get_or_insert(fresh);
                assert_eq!(fresh, kept, "U+{v:04X}");
                assert!(fresh.is_none_or(|page| page.holds(c)), "U+{v:04X}");
                assert_eq!(
                    fresh.is_some_and(|page| page.contains(c)),
                    map.contains(c),
                    "U+{v:04X}"
                );
                if let Some(before) = before {
                    assert!(!before.holds(c) && !before.contains(c), "U+{v:04X}");
                }
                chars.push(c);
            }
            before = kept.flatten();
        }
        let back_and_forth: Vec<char> = chars
            .iter()
            .zip(chars.iter().rev())
            .flat_map(|(&a, &b)| [a, b])
            .take(chars.len())
            .collect();
        for text in [&chars, &back_and_forth] {
            for len in 1..=8 {
                for cluster in text.windows(len).step_by(len) {
                    assert_eq!(
                        map.covers_all(cluster.iter().copied()),
                        cluster.iter().all(|&c| map.contains(c)),
                        "{cluster:?}"
                    );
                }
            }
        }
        assert!(map.covers_all([]));
    }

    #[test]
    fn a_page_and_a_cluster_answer_as_contains_does() {
        let mut builder = Builder::default();
        for (start, end) in [
            (0x20, 0x7E),
            (0xA0, 0x17F),
            (0x3000, 0x303F),
            (0x4E00, 0x4E3F),
            (0x1F600, 0x1F64F),
            (0x10FFFE, MAX),
        ] {
            builder.insert(start, end);
        }
        let map = builder.finish();
        agrees(&map, usize::MAX);
        assert!(map.page('\u{8000}').is_none());
        let latin = map.page('A').expect("page 0");
        assert!(latin.contains('A') && latin.contains('é'));
        assert!(latin.holds('\u{7F}') && !latin.contains('\u{7F}'));
        assert!(map.contains('中') && !latin.holds('中') && !latin.contains('中'));
        assert!(map.covers_all("Aé 。中😀".chars()));
        assert!(!map.covers_all("Aé 。中😀\u{7F}".chars()));
        assert!(!map.covers_all("\u{7F}A".chars()));
        agrees(&Charset::default(), usize::MAX);
        for subtables in [
            &[(3, 1, format4(&[(0x20, 0x7E), (0xA0, 0xFF)]))][..],
            &[(3, 10, format12(&[(0x41, 0x5A), (0x1F600, 0x1F64F)]))][..],
        ] {
            agrees(&charset(&font(subtables)), usize::MAX);
        }
    }

    /// The same, over the charsets of the fonts installed here.
    #[cfg(all(
        feature = "system",
        any(windows, all(unix, not(target_vendor = "apple")))
    ))]
    #[test]
    fn a_page_answers_as_contains_does_for_installed_fonts() {
        use crate::Layer;

        // CJK and emoji fonts first, for their hundreds of pages, where
        // they are installed; then whatever else the system lists.
        const FIRST: &[&str] = &[
            "Microsoft YaHei",
            "Yu Gothic",
            "Malgun Gothic",
            "Segoe UI Emoji",
            "Segoe UI Symbol",
            "Noto Sans CJK JP",
            "Noto Color Emoji",
            "DejaVu Sans",
        ];
        let collection = crate::Collection::system();
        let names = FIRST
            .iter()
            .copied()
            .chain(collection.layers().flat_map(Layer::names));
        let mut seen: Vec<*const Charset> = Vec::new();
        'families: for name in names {
            let Some(family) = collection.family(name) else {
                continue;
            };
            for font in family.fonts() {
                if seen.len() == 40 {
                    break 'families;
                }
                let map = font.charset();
                if !seen.contains(&(map as *const Charset)) {
                    seen.push(map);
                    agrees(map, 24);
                }
            }
        }
    }

    #[cfg(feature = "std")]
    mod on_disk {
        use super::*;
        use crate::font::sfnt::FileFont;
        use crate::test_fonts::{Temporary, font_collection};

        fn from_file(path: &std::path::Path, index: u32) -> Option<Charset> {
            FileFont::open(path, index).map(|mut font| super::super::from_file(&mut font).0)
        }

        #[test]
        fn reading_one_subtable_from_disk_agrees_with_reading_the_font() {
            let fonts = [
                font(&[(3, 1, format4(&[(0x20, 0x7E)]))]),
                font(&[
                    (3, 0, format4(&[(0xF020, 0xF0FF)])),
                    (3, 1, format4(&[(0x41, 0x5A)])),
                    (3, 10, format12(&[(0x41, 0x5A), (0x1F600, 0x1F64F)])),
                ]),
                font_with_tables(&[(*b"head", alloc::vec![0; 54])]),
            ];
            let file = Temporary::new("charsets.ttc", &font_collection(&fonts));
            for (index, font) in fonts.iter().enumerate() {
                assert_eq!(
                    from_file(file.path(), index as u32),
                    Some(charset(font)),
                    "font {index}"
                );
            }
            assert_eq!(from_file(file.path(), 3), None);
        }
    }
}
