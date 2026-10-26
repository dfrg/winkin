//! Generates `cleft`'s word dictionaries from ICU's word lists.
//!
//! `cargo run --release -p unicode_gen -- dictionaries` reads the lists
//! pinned in `data/icu-78.1rc/` (ICU4C's `brkitr/dictionaries` of the
//! release ICU4X 2.3's data was built from), checked against the manifest,
//! and writes one payload per list into
//! `crates/cleft/src/dictionary/data/`. Running it again
//! reproduces them byte for byte.
//!
//! Each payload is a minimal deterministic acyclic automaton (DAFSA) that
//! accepts exactly the list's words, in the format `cleft`'s
//! `dictionary` module documents and validates. Costs in `cjdict.txt` are
//! not kept: the segmenters take the longest match, as ICU4X does.
//!
//! # Building
//!
//! - **The words.** A line's text before any tab, trimmed; blank lines and
//!   lines starting with `#` are skipped, and so is the byte order mark.
//! - **The automaton.** A trie of the words, its states merged bottom up
//!   wherever two have the same finality and the same edges to the same
//!   states. That is the minimal automaton of a finite language.
//! - **The layout.** States in reverse postorder from the root, so the root
//!   comes first and every edge points forward. A state's last child in
//!   depth-first order often follows it straight away, and its edge then
//!   needs no bytes.
//! - **The check.** The generator walks the payload it wrote: every word is
//!   accepted, and the automaton accepts as many strings as there are
//!   words, so it accepts nothing else.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::pinned::read_pinned;

/// The folder of `data/` the word lists are pinned in: ICU's release tag.
const ICU_RELEASE: &str = "icu-78.1rc";

/// Where the payloads are written.
const OUT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/cleft/src/dictionary/data"
);

/// The payload's magic number.
const MAGIC: [u8; 4] = *b"WKDF";
/// The format version.
const VERSION: u16 = 1;
/// The header's length in bytes, before the source string.
const HEADER: usize = 24;
/// The edge count a state header holds in its low five bits; 31 says a
/// `u16` with the count less 31 follows.
const SHORT_COUNT: usize = 31;

/// One dictionary: its word list, its payload, its script byte and whether
/// its symbols are two bytes wide.
struct Source {
    list: &'static str,
    out: &'static str,
    script: u8,
    wide: bool,
}

/// The dictionaries, in the script byte's order.
const SOURCES: [Source; 5] = [
    Source {
        list: "thaidict.txt",
        out: "thai.dafsa",
        script: 1,
        wide: false,
    },
    Source {
        list: "laodict.txt",
        out: "lao.dafsa",
        script: 2,
        wide: false,
    },
    Source {
        list: "khmerdict.txt",
        out: "khmer.dafsa",
        script: 3,
        wide: false,
    },
    Source {
        list: "burmesedict.txt",
        out: "myanmar.dafsa",
        script: 4,
        wide: false,
    },
    Source {
        list: "cjdict.txt",
        out: "cj.dafsa",
        script: 5,
        wide: true,
    },
];

/// A state of the automaton: whether a word ends there, and its edges in
/// symbol order.
#[derive(Clone, PartialEq, Eq, Hash)]
struct State {
    terminal: bool,
    edges: Vec<(char, usize)>,
}

/// A trie node while the words are read.
#[derive(Default)]
struct TrieNode {
    terminal: bool,
    children: BTreeMap<char, TrieNode>,
}

/// Writes every payload, and prints what each holds.
///
/// Returns an error where a word list is missing or changed.
pub(crate) fn generate(data: &Path) -> Result<(), String> {
    let lists = read_pinned(data, ICU_RELEASE)?;
    std::fs::create_dir_all(OUT).expect("creating the payloads' folder");
    let mut total = 0;
    for source in &SOURCES {
        let bytes = lists
            .get(source.list)
            .unwrap_or_else(|| panic!("{} is not pinned", source.list));
        let text = std::str::from_utf8(bytes).expect("a word list is UTF-8");
        let words = read_words(text);
        let payload = encode(source, &words);
        check(source, &payload, &words);
        std::fs::write(Path::new(OUT).join(source.out), &payload).expect("writing a payload");
        total += payload.len();
    }
    println!("dictionaries: {total} bytes");
    Ok(())
}

/// Returns the words of an ICU word list.
fn read_words(text: &str) -> BTreeSet<Vec<char>> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut words = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let word = line.split('\t').next().unwrap_or_default().trim();
        let chars: Vec<char> = word.chars().collect();
        assert!(
            !chars.is_empty() && chars.iter().all(|ch| !ch.is_control() && *ch != ' '),
            "{word:?} is not a word"
        );
        words.insert(chars);
    }
    words
}

/// Returns the minimal automaton of `words`: its states, and the root's
/// index among them.
fn minimize(words: &BTreeSet<Vec<char>>) -> (Vec<State>, usize) {
    let mut root = TrieNode::default();
    for word in words {
        let mut node = &mut root;
        for &ch in word {
            node = node.children.entry(ch).or_default();
        }
        node.terminal = true;
    }
    let mut states = Vec::new();
    let mut ids = HashMap::new();
    let root = intern(&root, &mut states, &mut ids);
    (states, root)
}

/// Returns the index of the state `node` merges into, adding it if new.
fn intern(node: &TrieNode, states: &mut Vec<State>, ids: &mut HashMap<State, usize>) -> usize {
    let edges = node
        .children
        .iter()
        .map(|(&ch, child)| (ch, intern(child, states, ids)))
        .collect();
    let state = State {
        terminal: node.terminal,
        edges,
    };
    if let Some(&id) = ids.get(&state) {
        return id;
    }
    let id = states.len();
    states.push(state.clone());
    ids.insert(state, id);
    id
}

/// Returns the states in reverse postorder from `root`: each before every
/// state it reaches.
fn layout(states: &[State], root: usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(states.len());
    let mut seen = vec![false; states.len()];
    let mut stack = vec![(root, 0)];
    seen[root] = true;
    while let Some((id, next)) = stack.last_mut() {
        let edges = &states[*id].edges;
        if let Some(&(_, child)) = edges.get(*next) {
            *next += 1;
            if !seen[child] {
                seen[child] = true;
                stack.push((child, 0));
            }
        } else {
            order.push(*id);
            stack.pop();
        }
    }
    order.reverse();
    order
}

/// Returns the alphabet and each character's symbol: for one-byte symbols,
/// every character in code point order; for two-byte symbols, the
/// characters past the BMP, whose symbols are 0xD800 and up, while a BMP
/// character's symbol is its code point.
fn alphabet(states: &[State], wide: bool) -> (Vec<u32>, BTreeMap<char, u16>) {
    let chars: BTreeSet<char> = states
        .iter()
        .flat_map(|state| state.edges.iter().map(|&(ch, _)| ch))
        .collect();
    let mut listed = Vec::new();
    let mut symbols = BTreeMap::new();
    for ch in chars {
        let cp = u32::from(ch);
        if wide && cp < 0x1_0000 {
            symbols.insert(ch, cp as u16);
            continue;
        }
        let index = u16::try_from(listed.len()).expect("a small alphabet");
        let symbol = if wide { 0xD800 + index } else { index };
        assert!(
            (!wide && index < 256) || (wide && symbol < 0xE000),
            "the alphabet is too large for the symbol width"
        );
        listed.push(cp);
        symbols.insert(ch, symbol);
    }
    (listed, symbols)
}

/// Returns the number of bytes `value` needs, 0 for 0.
fn width(value: usize) -> usize {
    match value {
        0 => 0,
        1..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFF_FFFF => 3,
        _ => panic!("an edge spans more than 16 MiB"),
    }
}

/// Returns the payload of `words`.
fn encode(source: &Source, words: &BTreeSet<Vec<char>>) -> Vec<u8> {
    let (states, root) = minimize(words);
    let order = layout(&states, root);
    let (listed, symbols) = alphabet(&states, source.wide);
    let symbol_width = if source.wide { 2 } else { 1 };
    // Each state's record: a header, the symbols and the targets. The target
    // width is the least that holds every delta, so the sizes shrink until
    // they hold still.
    let header = |state: &State| {
        1 + if state.edges.len() >= SHORT_COUNT {
            2
        } else {
            0
        }
    };
    let mut target_widths: Vec<usize> = vec![3; states.len()];
    let mut offsets = vec![0; states.len()];
    loop {
        let mut at = 0;
        for &id in &order {
            offsets[id] = at;
            let state = &states[id];
            at += header(state) + state.edges.len() * (symbol_width + target_widths[id]);
        }
        let mut changed = false;
        for &id in &order {
            let state = &states[id];
            let end = offsets[id]
                + header(state)
                + state.edges.len() * (symbol_width + target_widths[id]);
            let widest = state
                .edges
                .iter()
                .map(|&(_, child)| width(offsets[child] - end))
                .max()
                .unwrap_or(0);
            if widest != target_widths[id] {
                assert!(widest < target_widths[id], "a record grew");
                target_widths[id] = widest;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut records = Vec::new();
    for &id in &order {
        let state = &states[id];
        assert_eq!(offsets[id], records.len());
        let count = state.edges.len();
        let target_width = target_widths[id];
        let mut first = (u8::from(state.terminal) << 7) | ((target_width as u8) << 5);
        if count >= SHORT_COUNT {
            first |= SHORT_COUNT as u8;
            records.push(first);
            let extra = u16::try_from(count - SHORT_COUNT).expect("fewer than 2^16 edges");
            records.extend_from_slice(&extra.to_le_bytes());
        } else {
            records.push(first | count as u8);
        }
        let mut edges: Vec<(u16, usize)> = state
            .edges
            .iter()
            .map(|&(ch, child)| (symbols[&ch], child))
            .collect();
        edges.sort_unstable();
        for &(symbol, _) in &edges {
            records.extend_from_slice(&symbol.to_le_bytes()[..symbol_width]);
        }
        let end = records.len() + count * target_width;
        for &(_, child) in &edges {
            let delta = (offsets[child] - end) as u32;
            records.extend_from_slice(&delta.to_le_bytes()[..target_width]);
        }
    }
    let source_name = format!("ICU {ICU_RELEASE} {}", source.list);
    let mut payload = Vec::new();
    payload.extend_from_slice(&MAGIC);
    payload.extend_from_slice(&VERSION.to_le_bytes());
    payload.push(source.script);
    payload.push(symbol_width as u8);
    let length = HEADER + source_name.len() + 4 * listed.len() + records.len();
    payload.extend_from_slice(&u32::try_from(length).expect("under 4 GiB").to_le_bytes());
    // The flags: none in this version.
    payload.extend_from_slice(&0u16.to_le_bytes());
    let source_length = u16::try_from(source_name.len()).expect("a short source");
    payload.extend_from_slice(&source_length.to_le_bytes());
    let alphabet_length = u16::try_from(listed.len()).expect("a small alphabet");
    payload.extend_from_slice(&alphabet_length.to_le_bytes());
    payload.extend_from_slice(&0u16.to_le_bytes());
    let records_length = u32::try_from(records.len()).expect("under 4 GiB");
    payload.extend_from_slice(&records_length.to_le_bytes());
    assert_eq!(payload.len(), HEADER);
    payload.extend_from_slice(source_name.as_bytes());
    for cp in &listed {
        payload.extend_from_slice(&cp.to_le_bytes());
    }
    payload.extend_from_slice(&records);
    assert_eq!(payload.len(), length);
    let edges: usize = states.iter().map(|state| state.edges.len()).sum();
    println!(
        "{:14} {:7} words {:7} states {:7} edges, alphabet {:5}, {:9} bytes",
        source.out,
        words.len(),
        states.len(),
        edges,
        if source.wide {
            symbols.len()
        } else {
            listed.len()
        },
        payload.len()
    );
    payload
}

/// A payload read back, for the check.
struct Reader<'a> {
    records: &'a [u8],
    wide: bool,
    symbols: BTreeMap<char, u16>,
}

impl Reader<'_> {
    /// Returns a record's finality, its symbols and their targets.
    fn record(&self, at: usize) -> (bool, Vec<(u16, usize)>, usize) {
        let first = self.records[at];
        let terminal = first & 0x80 != 0;
        let target_width = usize::from((first >> 5) & 3);
        let mut count = usize::from(first & 0x1F);
        let mut next = at + 1;
        if count == SHORT_COUNT {
            count += usize::from(u16::from_le_bytes([
                self.records[next],
                self.records[next + 1],
            ]));
            next += 2;
        }
        let symbol_width = if self.wide { 2 } else { 1 };
        let symbols_at = next;
        let targets_at = symbols_at + count * symbol_width;
        let end = targets_at + count * target_width;
        let edges = (0..count)
            .map(|edge| {
                let s = symbols_at + edge * symbol_width;
                let symbol = if self.wide {
                    u16::from_le_bytes([self.records[s], self.records[s + 1]])
                } else {
                    u16::from(self.records[s])
                };
                let t = targets_at + edge * target_width;
                let mut delta = [0u8; 4];
                delta[..target_width].copy_from_slice(&self.records[t..t + target_width]);
                (symbol, end + u32::from_le_bytes(delta) as usize)
            })
            .collect();
        (terminal, edges, end)
    }

    /// Returns whether the payload accepts `word`.
    fn accepts(&self, word: &[char]) -> bool {
        let mut at = 0;
        for ch in word {
            let Some(&symbol) = self.symbols.get(ch) else {
                return false;
            };
            let (_, edges, _) = self.record(at);
            match edges.iter().find(|&&(found, _)| found == symbol) {
                Some(&(_, target)) => at = target,
                None => return false,
            }
        }
        self.record(at).0
    }
}

/// Checks that `payload` accepts exactly `words`.
fn check(source: &Source, payload: &[u8], words: &BTreeSet<Vec<char>>) {
    let source_length = usize::from(u16::from_le_bytes([payload[14], payload[15]]));
    let alphabet_length = usize::from(u16::from_le_bytes([payload[16], payload[17]]));
    let records_at = HEADER + source_length + 4 * alphabet_length;
    let mut symbols = BTreeMap::new();
    for (index, chunk) in payload[HEADER + source_length..records_at]
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
    {
        let cp = u32::from_le_bytes(*chunk);
        let ch = char::from_u32(cp).expect("a scalar value");
        let symbol = if source.wide { 0xD800 + index } else { index };
        symbols.insert(ch, symbol as u16);
    }
    if source.wide {
        for word in words {
            for &ch in word {
                if u32::from(ch) < 0x1_0000 {
                    symbols.insert(ch, u32::from(ch) as u16);
                }
            }
        }
    }
    let reader = Reader {
        records: &payload[records_at..],
        wide: source.wide,
        symbols,
    };
    for word in words {
        assert!(reader.accepts(word), "{} leaves out a word", source.out);
    }
    // The number of strings each record's state accepts, from the last
    // record back: every edge points forward.
    let mut starts = Vec::new();
    let mut at = 0;
    while at < reader.records.len() {
        starts.push(at);
        at = reader.record(at).2;
    }
    let mut accepted: HashMap<usize, u64> = HashMap::new();
    for &start in starts.iter().rev() {
        let (terminal, edges, _) = reader.record(start);
        let below: u64 = edges.iter().map(|(_, target)| accepted[target]).sum();
        accepted.insert(start, below + u64::from(terminal));
    }
    assert_eq!(
        accepted[&0],
        words.len() as u64,
        "{} accepts strings that are not words",
        source.out
    );
}
