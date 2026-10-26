//! Word dictionaries: the automata that find the words of Thai, Lao, Khmer
//! and Myanmar text, and of Chinese and Japanese, where no rule can.
//!
//! A [`Dictionary`] is one script's word list as a deterministic acyclic
//! automaton, validated once when it is made. [`Dictionaries`] holds one per
//! script a host supplies; the `dictionaries` feature compiles in all five,
//! generated from ICU's word lists.
//!
//! `matcher` finds the longest word at a position, and `run` follows a run
//! of characters a dictionary segments, as line and word segmentation read
//! them.
//!
//! # Format
//!
//! A dictionary is a byte string, little-endian throughout:
//!
//! | Offset | Size | Field |
//! |---:|---:|---|
//! | 0 | 4 | magic number, `WKDF` |
//! | 4 | 2 | format version, 1 |
//! | 6 | 1 | script: 1 Thai, 2 Lao, 3 Khmer, 4 Myanmar, 5 Chinese and Japanese |
//! | 7 | 1 | symbol width in bytes, 1 or 2 |
//! | 8 | 4 | length of the whole dictionary in bytes |
//! | 12 | 2 | flags, 0: no other value is defined in version 1 |
//! | 14 | 2 | source length *S* in bytes |
//! | 16 | 2 | alphabet length *A* in entries |
//! | 18 | 2 | reserved, 0 |
//! | 20 | 4 | automaton length *T* in bytes |
//! | 24 | *S* | source: what the words were generated from, in ASCII |
//! | 24 + *S* | 4 *A* | alphabet: code points, strictly increasing |
//! | 24 + *S* + 4 *A* | *T* | automaton |
//!
//! **Symbols.** Each edge of the automaton is labelled with a symbol, one
//! character of a word:
//! - with one-byte symbols, a character's symbol is its index in the
//!   alphabet, which lists at most 256 characters;
//! - with two-byte symbols, a character of the BMP is its own symbol, and one
//!   past the BMP is 0xD800 plus its index in the alphabet, which lists only
//!   those, at most 2048.
//!
//! **The automaton** is a sequence of state records, the root first. Every
//! edge points to a record after its own, so the automaton has no cycle. A
//! record is:
//! - a header byte: bit 7 is set where a word ends at the state; bits 5 and
//!   6 are the width *W* of each target, 0 to 3 bytes; bits 0 to 4 are the
//!   number of edges *N*, where 31 means a `u16` follows holding *N* less 31;
//! - *N* symbols, strictly increasing;
//! - *N* targets, each *W* bytes: the distance in bytes from the end of the
//!   record to the start of the record its edge leads to.
//!
//! A record's first child in the layout often follows it at once, at
//! distance 0, and needs no target bytes where all its edges do.
//!
//! Version 1 records no values, such as the costs ICU4C's Chinese and
//! Japanese breaking weighs. A later version can add them behind a flag.
//!
//! # Validation
//!
//! [`Dictionary::new`] checks the header and the alphabet, then walks every
//! record once: each lies within the automaton, its symbols increase and
//! name characters of the alphabet, and each target is the start of a later
//! record. A scan then reads only records it has reached by edges, so every
//! read is in bounds and the matcher's loop has no failing path.

mod matcher;
mod run;
#[cfg(test)]
mod tests;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

pub(crate) use matcher::next_word;
pub(crate) use run::{Run, RunScript, find_next_cj_word, find_next_word, is_katakana};

/// The magic number a dictionary starts with.
const MAGIC: [u8; 4] = *b"WKDF";
/// The format version this crate reads.
const VERSION: u16 = 1;
/// The length of the header before the source string.
const HEADER: usize = 24;
/// The edge count a record header holds in its low five bits; 31 says a
/// `u16` with the count less 31 follows.
const SHORT_COUNT: usize = 31;
/// The bit of a record header set where a word ends at the state.
const TERMINAL: u8 = 0x80;
/// The first two-byte symbol of a character past the BMP.
const SUPPLEMENTARY: u16 = 0xD800;
/// The most characters past the BMP a dictionary of two-byte symbols lists.
const MOST_SUPPLEMENTARY: usize = 0x800;

/// The bytes of a dictionary: compiled in, or shared.
#[derive(Clone, Debug)]
pub enum Bytes {
    /// Bytes that live for the whole program, such as compiled-in data.
    Static(&'static [u8]),
    /// Bytes shared by reference count, such as a file loaded at run time.
    Shared(Arc<[u8]>),
}

impl Bytes {
    /// Returns the bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

impl From<&'static [u8]> for Bytes {
    fn from(bytes: &'static [u8]) -> Self {
        Self::Static(bytes)
    }
}

impl From<Arc<[u8]>> for Bytes {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self::Shared(bytes)
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Shared(bytes.into())
    }
}

/// The script a dictionary finds words in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DictionaryScript {
    /// Thai.
    Thai,
    /// Lao.
    Lao,
    /// Khmer.
    Khmer,
    /// Myanmar (Burmese).
    Myanmar,
    /// Chinese and Japanese: Han, Hiragana and Katakana.
    ChineseJapanese,
}

impl DictionaryScript {
    /// Returns the script a header's script byte names.
    const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Thai,
            2 => Self::Lao,
            3 => Self::Khmer,
            4 => Self::Myanmar,
            5 => Self::ChineseJapanese,
            _ => return None,
        })
    }
}

/// Why bytes are not a dictionary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DictionaryError {
    /// The bytes are shorter than the header, or do not start with the magic
    /// number.
    Magic,
    /// The format version is not one this crate reads.
    Version(u16),
    /// The script byte names no script.
    Script(u8),
    /// The header's length, flags, symbol width or field lengths do not fit
    /// the bytes.
    Header,
    /// The source is not ASCII, or the alphabet is not strictly increasing
    /// characters that fit the symbol width.
    Alphabet,
    /// The state record at this offset of the automaton runs past its end,
    /// or its symbols do not increase or name no character.
    State(usize),
    /// An edge of the state record at this offset of the automaton does not
    /// lead to the start of a later record.
    Edge(usize),
    /// The dictionary is for another script than the one asked for.
    WrongScript {
        /// The script asked for.
        expected: DictionaryScript,
        /// The dictionary's script.
        found: DictionaryScript,
    },
}

impl fmt::Display for DictionaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Magic => f.write_str("not a dictionary: no magic number"),
            Self::Version(version) => write!(f, "dictionary format version {version} is unknown"),
            Self::Script(script) => write!(f, "dictionary script {script} is unknown"),
            Self::Header => f.write_str("the dictionary header does not fit its bytes"),
            Self::Alphabet => f.write_str("the dictionary source or alphabet is malformed"),
            Self::State(at) => write!(f, "the dictionary state at byte {at} is malformed"),
            Self::Edge(at) => write!(f, "an edge of the dictionary state at byte {at} is bad"),
            Self::WrongScript { expected, found } => {
                write!(f, "a {found:?} dictionary where a {expected:?} one belongs")
            }
        }
    }
}

impl core::error::Error for DictionaryError {}

/// One script's words, as a validated automaton.
///
/// Matching reads the bytes in place, at the same speed whether they are
/// compiled in or loaded at run time.
#[derive(Clone, Debug)]
pub struct Dictionary {
    bytes: Bytes,
    script: DictionaryScript,
    /// Whether symbols are two bytes wide.
    wide: bool,
    /// Where the source string ends in the bytes, and the alphabet starts.
    alphabet: usize,
    /// Where the automaton starts in the bytes, after the alphabet.
    records: usize,
    /// The first code point of the window `window` maps.
    window_start: u32,
    /// For one-byte symbols, each code point from `window_start` on: its
    /// symbol plus one, or 0 where the alphabet lacks it.
    window: [u8; 256],
}

impl Dictionary {
    /// Creates a dictionary from `bytes`, after checking that they are one.
    ///
    /// The check reads every byte once and allocates a bit per byte of the
    /// automaton while it runs.
    ///
    /// # Errors
    ///
    /// Returns the first fault found, where the bytes are not a dictionary of
    /// the format version this crate reads.
    pub fn new(bytes: impl Into<Bytes>) -> Result<Self, DictionaryError> {
        let dictionary = Self::from_header(bytes.into())?;
        dictionary.check_records()?;
        Ok(dictionary)
    }

    /// Returns the script the dictionary finds words in.
    #[inline]
    pub fn script(&self) -> DictionaryScript {
        self.script
    }

    /// Returns what the words were generated from, such as
    /// `"ICU icu-78.1rc thaidict.txt"`.
    pub fn source(&self) -> &str {
        let bytes = self.bytes.as_slice().get(HEADER..self.alphabet);
        // The source was checked to be ASCII.
        bytes
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or_default()
    }

    /// Returns the dictionary's bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Returns whether `word` is one of the dictionary's words.
    pub fn contains(&self, word: &str) -> bool {
        let automaton = self.automaton();
        let mut state = 0;
        for ch in word.chars() {
            match self
                .symbol(ch)
                .and_then(|symbol| automaton.step(state, symbol))
            {
                Some(next) => state = next,
                None => return false,
            }
        }
        automaton.is_terminal(state)
    }

    /// Creates a dictionary from bytes this crate compiles in, which a test
    /// checks, without walking the automaton.
    #[cfg(feature = "dictionaries")]
    fn from_compiled(bytes: &'static [u8]) -> Option<Self> {
        Self::from_header(Bytes::Static(bytes)).ok()
    }

    /// Reads and checks the header and the alphabet.
    fn from_header(bytes: Bytes) -> Result<Self, DictionaryError> {
        let data = bytes.as_slice();
        if data.len() < HEADER || data.get(..4) != Some(&MAGIC[..]) {
            return Err(DictionaryError::Magic);
        }
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let u32_at =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let version = u16_at(4);
        if version != VERSION {
            return Err(DictionaryError::Version(version));
        }
        let script =
            DictionaryScript::from_byte(data[6]).ok_or(DictionaryError::Script(data[6]))?;
        let wide = match data[7] {
            1 => false,
            2 => true,
            _ => return Err(DictionaryError::Header),
        };
        let length = u32_at(8) as usize;
        let (flags, source_length, alphabet_length, reserved) =
            (u16_at(12), u16_at(14), u16_at(16), u16_at(18));
        let records_length = u32_at(20) as usize;
        let alphabet = HEADER + usize::from(source_length);
        let records = alphabet + 4 * usize::from(alphabet_length);
        if flags != 0
            || reserved != 0
            || length != data.len()
            || records.checked_add(records_length) != Some(length)
            || records_length == 0
        {
            return Err(DictionaryError::Header);
        }
        let most = if wide { MOST_SUPPLEMENTARY } else { 256 };
        if usize::from(alphabet_length) > most
            || !data.get(HEADER..alphabet).is_some_and(<[u8]>::is_ascii)
        {
            return Err(DictionaryError::Alphabet);
        }
        let mut window_start = 0;
        let mut window = [0; 256];
        let mut previous = None;
        for (index, chunk) in data
            .get(alphabet..records)
            .unwrap_or_default()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let cp = u32::from_le_bytes(*chunk);
            let valid = char::from_u32(cp).is_some()
                && previous.is_none_or(|previous| cp > previous)
                && (!wide || cp >= 0x1_0000);
            if !valid {
                return Err(DictionaryError::Alphabet);
            }
            if index == 0 {
                window_start = cp;
            }
            if !wide && let Some(slot) = window.get_mut(cp.wrapping_sub(window_start) as usize) {
                // The index is under 256, so the symbol plus one fits.
                *slot = index as u8 + 1;
            }
            previous = Some(cp);
        }
        Ok(Self {
            bytes,
            script,
            wide,
            alphabet,
            records,
            window_start,
            window,
        })
    }

    /// Walks every record of the automaton, checking that each lies within it,
    /// that its symbols increase and name characters, and that every edge
    /// leads to the start of a later record.
    fn check_records(&self) -> Result<(), DictionaryError> {
        let records = self.records_slice();
        let symbol_width = if self.wide { 2 } else { 1 };
        let alphabet_length = (self.records - self.alphabet) / 4;
        // One bit per byte of the automaton: where a record starts.
        let mut starts = vec![0u64; records.len().div_ceil(64)];
        let mut at = 0;
        while at < records.len() {
            let Some(layout) = record_layout(records, at, symbol_width) else {
                return Err(DictionaryError::State(at));
            };
            let mut previous = None;
            for edge in 0..layout.count {
                let symbol = read_symbol(records, layout.symbols + edge * symbol_width, self.wide);
                let named = if self.wide {
                    !(SUPPLEMENTARY..0xE000).contains(&symbol)
                        || usize::from(symbol - SUPPLEMENTARY) < alphabet_length
                } else {
                    usize::from(symbol) < alphabet_length
                };
                if !named || previous.is_some_and(|previous| symbol <= previous) {
                    return Err(DictionaryError::State(at));
                }
                previous = Some(symbol);
            }
            starts[at / 64] |= 1 << (at % 64);
            at = layout.end;
        }
        let is_start = |at: usize| {
            starts
                .get(at / 64)
                .is_some_and(|bits| bits & (1 << (at % 64)) != 0)
        };
        let mut at = 0;
        while at < records.len() {
            let Some(layout) = record_layout(records, at, symbol_width) else {
                return Err(DictionaryError::State(at));
            };
            for edge in 0..layout.count {
                let target = read_target(records, &layout, edge);
                if !is_start(target) {
                    return Err(DictionaryError::Edge(at));
                }
            }
            at = layout.end;
        }
        Ok(())
    }

    /// Returns the automaton's records.
    #[inline]
    fn records_slice(&self) -> &[u8] {
        self.bytes
            .as_slice()
            .get(self.records..)
            .unwrap_or_default()
    }

    /// Returns the automaton, to read.
    #[inline]
    pub(crate) fn automaton(&self) -> Automaton<'_> {
        Automaton {
            records: self.records_slice(),
            wide: self.wide,
        }
    }

    /// Returns the symbol of `ch`, or `None` where no word has it.
    #[inline]
    pub(crate) fn symbol(&self, ch: char) -> Option<u16> {
        let cp = u32::from(ch);
        if self.wide {
            if cp < 0x1_0000 {
                return Some(cp as u16);
            }
            let index = self.alphabet_index(cp)?;
            return Some(SUPPLEMENTARY + index as u16);
        }
        if let Some(&slot) = self.window.get(cp.wrapping_sub(self.window_start) as usize) {
            return slot.checked_sub(1).map(u16::from);
        }
        self.alphabet_index(cp).map(|index| index as u16)
    }

    /// Returns the index of `cp` in the alphabet.
    #[cold]
    fn alphabet_index(&self, cp: u32) -> Option<usize> {
        let alphabet = self
            .bytes
            .as_slice()
            .get(self.alphabet..self.records)
            .unwrap_or_default();
        let count = alphabet.len() / 4;
        let (mut low, mut high) = (0, count);
        while low < high {
            let middle = (low + high) / 2;
            let at = middle * 4;
            let found = u32::from_le_bytes([
                alphabet[at],
                alphabet[at + 1],
                alphabet[at + 2],
                alphabet[at + 3],
            ]);
            match found.cmp(&cp) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }
}

/// Where the parts of one record lie.
struct RecordLayout {
    count: usize,
    target_width: usize,
    symbols: usize,
    targets: usize,
    end: usize,
}

/// Returns the layout of the record at `at` of `records`, or `None` where it
/// runs past their end.
fn record_layout(records: &[u8], at: usize, symbol_width: usize) -> Option<RecordLayout> {
    let first = *records.get(at)?;
    let mut count = usize::from(first & 0x1F);
    let mut symbols = at + 1;
    if count == SHORT_COUNT {
        let extra = records.get(symbols..symbols + 2)?;
        count += usize::from(u16::from_le_bytes([extra[0], extra[1]]));
        symbols += 2;
    }
    let target_width = usize::from((first >> 5) & 3);
    let targets = symbols + count * symbol_width;
    let end = targets + count * target_width;
    (end <= records.len()).then_some(RecordLayout {
        count,
        target_width,
        symbols,
        targets,
        end,
    })
}

/// Reads the symbol at `at` of `records`.
#[inline(always)]
fn read_symbol(records: &[u8], at: usize, wide: bool) -> u16 {
    if wide {
        u16::from_le_bytes([records[at], records[at + 1]])
    } else {
        u16::from(records[at])
    }
}

/// Returns where edge `edge` of the record `layout` leads.
#[inline(always)]
fn read_target(records: &[u8], layout: &RecordLayout, edge: usize) -> usize {
    let at = layout.targets + edge * layout.target_width;
    let mut distance = 0;
    for byte in 0..layout.target_width {
        distance |= usize::from(records[at + byte]) << (8 * byte);
    }
    layout.end + distance
}

/// A validated automaton, read in place.
///
/// Every record a read reaches lies within `records`, since validation
/// checked each one and every edge's target, so its indexing never fails.
#[derive(Copy, Clone)]
pub(crate) struct Automaton<'a> {
    records: &'a [u8],
    wide: bool,
}

impl Automaton<'_> {
    /// Returns the state the edge labelled `symbol` leads to from `state`,
    /// or `None` where it has none.
    #[inline]
    pub(crate) fn step(self, state: usize, symbol: u16) -> Option<usize> {
        let symbol_width = if self.wide { 2 } else { 1 };
        let layout = self.layout(state, symbol_width);
        let (mut low, mut high) = (0, layout.count);
        while low < high {
            let middle = (low + high) / 2;
            let found = read_symbol(
                self.records,
                layout.symbols + middle * symbol_width,
                self.wide,
            );
            match found.cmp(&symbol) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => {
                    return Some(read_target(self.records, &layout, middle));
                }
            }
        }
        None
    }

    /// Returns whether a word ends at `state`.
    #[inline]
    pub(crate) fn is_terminal(self, state: usize) -> bool {
        self.records[state] & TERMINAL != 0
    }

    /// Returns whether `state` has no edge, so no longer word passes it.
    #[cfg(test)]
    pub(crate) fn is_leaf(self, state: usize) -> bool {
        self.records[state] & 0x1F == 0
    }

    /// Returns the layout of the record of `state`, which validation proved
    /// lies within the records.
    #[inline(always)]
    fn layout(self, state: usize, symbol_width: usize) -> RecordLayout {
        let first = self.records[state];
        let mut count = usize::from(first & 0x1F);
        let mut symbols = state + 1;
        if count == SHORT_COUNT {
            count += usize::from(u16::from_le_bytes([
                self.records[symbols],
                self.records[symbols + 1],
            ]));
            symbols += 2;
        }
        let target_width = usize::from((first >> 5) & 3);
        let targets = symbols + count * symbol_width;
        RecordLayout {
            count,
            target_width,
            symbols,
            targets,
            end: targets + count * target_width,
        }
    }
}

/// The word dictionaries a segmenter reads, one per script, each optional.
///
/// Text in a script with no dictionary is not broken inside its runs. With
/// the `dictionaries` feature, `Dictionaries::with_compiled` holds all five.
#[derive(Clone, Debug, Default)]
pub struct Dictionaries {
    thai: Option<Dictionary>,
    lao: Option<Dictionary>,
    khmer: Option<Dictionary>,
    myanmar: Option<Dictionary>,
    cj: Option<Dictionary>,
}

impl Dictionaries {
    /// Creates a set with no dictionaries.
    pub const fn new() -> Self {
        Self {
            thai: None,
            lao: None,
            khmer: None,
            myanmar: None,
            cj: None,
        }
    }

    /// Creates the set compiled into the crate: Thai, Lao, Khmer, Myanmar,
    /// and Chinese and Japanese, generated from ICU's word lists (about
    /// 2.7 MB).
    #[cfg(feature = "dictionaries")]
    pub fn with_compiled() -> Self {
        Self {
            thai: Dictionary::from_compiled(include_bytes!("data/thai.dafsa")),
            lao: Dictionary::from_compiled(include_bytes!("data/lao.dafsa")),
            khmer: Dictionary::from_compiled(include_bytes!("data/khmer.dafsa")),
            myanmar: Dictionary::from_compiled(include_bytes!("data/myanmar.dafsa")),
            cj: Dictionary::from_compiled(include_bytes!("data/cj.dafsa")),
        }
    }

    /// Returns the set with `dictionary` as its Thai dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`DictionaryError::WrongScript`] where `dictionary` is not
    /// Thai.
    pub fn with_thai(mut self, dictionary: Dictionary) -> Result<Self, DictionaryError> {
        self.thai = Some(expect(dictionary, DictionaryScript::Thai)?);
        Ok(self)
    }

    /// Returns the set with `dictionary` as its Lao dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`DictionaryError::WrongScript`] where `dictionary` is not
    /// Lao.
    pub fn with_lao(mut self, dictionary: Dictionary) -> Result<Self, DictionaryError> {
        self.lao = Some(expect(dictionary, DictionaryScript::Lao)?);
        Ok(self)
    }

    /// Returns the set with `dictionary` as its Khmer dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`DictionaryError::WrongScript`] where `dictionary` is not
    /// Khmer.
    pub fn with_khmer(mut self, dictionary: Dictionary) -> Result<Self, DictionaryError> {
        self.khmer = Some(expect(dictionary, DictionaryScript::Khmer)?);
        Ok(self)
    }

    /// Returns the set with `dictionary` as its Myanmar dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`DictionaryError::WrongScript`] where `dictionary` is not
    /// Myanmar.
    pub fn with_myanmar(mut self, dictionary: Dictionary) -> Result<Self, DictionaryError> {
        self.myanmar = Some(expect(dictionary, DictionaryScript::Myanmar)?);
        Ok(self)
    }

    /// Returns the set with `dictionary` as its Chinese and Japanese
    /// dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`DictionaryError::WrongScript`] where `dictionary` is not for
    /// Chinese and Japanese.
    pub fn with_chinese_japanese(
        mut self,
        dictionary: Dictionary,
    ) -> Result<Self, DictionaryError> {
        self.cj = Some(expect(dictionary, DictionaryScript::ChineseJapanese)?);
        Ok(self)
    }

    /// Returns the dictionary of `script`, if the set has one.
    pub fn get(&self, script: DictionaryScript) -> Option<&Dictionary> {
        match script {
            DictionaryScript::Thai => self.thai.as_ref(),
            DictionaryScript::Lao => self.lao.as_ref(),
            DictionaryScript::Khmer => self.khmer.as_ref(),
            DictionaryScript::Myanmar => self.myanmar.as_ref(),
            DictionaryScript::ChineseJapanese => self.cj.as_ref(),
        }
    }

    /// Returns whether one of the dictionaries finds the words of a run of
    /// `ch`: a Thai, Lao, Khmer or Myanmar character (Line_Break SA) where
    /// the set has its script's dictionary, or a Han, Hiragana or Katakana
    /// one where it has the Chinese and Japanese dictionary.
    ///
    /// A word boundary inside such a run depends on the run from its start.
    ///
    /// # Examples
    ///
    /// ```
    /// use cleft::Dictionaries;
    ///
    /// assert!(!Dictionaries::new().segments('ก'));
    /// assert!(!Dictionaries::new().segments('a'));
    /// ```
    pub fn segments(&self, ch: char) -> bool {
        crate::word::run_script(ch)
            .and_then(|script| script.find(Some(self)))
            .is_some()
    }

    /// Returns whether the set has no dictionary.
    pub fn is_empty(&self) -> bool {
        self.thai.is_none()
            && self.lao.is_none()
            && self.khmer.is_none()
            && self.myanmar.is_none()
            && self.cj.is_none()
    }
}

/// Returns `dictionary` if it is for `script`.
fn expect(dictionary: Dictionary, script: DictionaryScript) -> Result<Dictionary, DictionaryError> {
    if dictionary.script == script {
        Ok(dictionary)
    } else {
        Err(DictionaryError::WrongScript {
            expected: script,
            found: dictionary.script,
        })
    }
}
