//! What the sweeps share: the pinned UCD files of the tables' version and of
//! ICU's, read so that a sweep can tell a difference between the two
//! versions from a mistake.
//!
//! icu_properties and icu_casemap 2.3 hold Unicode 17.0 data, and the
//! tables Unicode 18.0. A sweep compares the tables with ICU wherever the
//! two versions' files agree, and with the 18.0 files where they differ,
//! checking that ICU holds the 17.0 value there. So every difference from
//! ICU is one between the two versions' files, and its count is pinned.
//!
//! The files are `support/unicode_gen/data`'s, which
//! `cargo run -p unicode_gen -- fetch` downloads, read by a parser of the
//! tests' own, written apart from the generator's. The break test files
//! (GraphemeBreakTest.txt, LineBreakTest.txt) are read by
//! [`break_test_cases`].

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{eprintln, fs, vec};

mod runs;

pub(crate) use runs::{
    RunQuirks, SA_LISTS, causes_restoring, icu_run_breaks, list_words, xorshift,
};

use crate::{GraphemeClusterBreak, GraphemeProps};

/// Counts each test thread's allocations, for the tests that pin none.
#[global_allocator]
static GLOBAL: testing::allocator::Counting = testing::allocator::Counting;

/// The version of icu_properties' and icu_casemap's data.
pub(crate) const ICU_VERSION: &str = "17.0.0";

/// The version of the tables.
pub(crate) const TABLES_VERSION: &str = "18.0.0";

/// The number of code points.
const CODE_POINTS: usize = 0x11_0000;

/// Returns the directory of `version`'s pinned files.
fn version_dir(version: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../support/unicode_gen/data")
        .join(version)
}

/// Returns the text of `version`'s pinned file `file`, or `None` where it is
/// absent, as in a packaged crate or a checkout that has not fetched the
/// files (`cargo run -p unicode_gen -- fetch`).
pub(crate) fn pinned_file(version: &str, file: &str) -> Option<String> {
    let path = version_dir(version).join(file);
    let text = fs::read_to_string(&path);
    if text.is_err() {
        eprintln!(
            "{} is absent: the test is skipped (`cargo run -p unicode_gen -- fetch` fetches it)",
            path.display()
        );
    }
    text.ok()
}

/// Returns each case of a break test file: its text, and the byte offsets
/// of the boundaries the file marks, both ends included.
pub(crate) fn break_test_cases(file: &str) -> Vec<(String, Vec<usize>)> {
    let mut cases = Vec::new();
    for line in file.lines() {
        let data = line.split('#').next().unwrap_or_default().trim();
        if data.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut boundaries = Vec::new();
        for token in data.split_whitespace() {
            match token {
                "÷" => boundaries.push(text.len()),
                "×" => {}
                hex => text.push(
                    u32::from_str_radix(hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .expect("a scalar value"),
                ),
            }
        }
        cases.push((text, boundaries));
    }
    cases
}

/// One version's files.
pub(crate) struct Ucd {
    dir: PathBuf,
    aliases: HashMap<(String, String), String>,
}

/// Returns `name` in UAX44-LM3's loose form.
fn loose(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '-' | '_'))
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match name.strip_prefix("is") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => name,
    }
}

/// Returns the code points of `XXXX` or `XXXX..YYYY`.
fn range(text: &str) -> core::ops::RangeInclusive<usize> {
    let text = text.trim();
    let parse = |hex: &str| usize::from_str_radix(hex, 16).expect("a code point");
    match text.split_once("..") {
        Some((start, end)) => parse(start)..=parse(end),
        None => parse(text)..=parse(text),
    }
}

/// Returns the fields of each data line of `text`, comments dropped.
fn lines(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines().filter_map(|line| {
        let data = line.split('#').next()?.trim();
        (!data.is_empty()).then(|| data.split(';').map(str::trim).collect())
    })
}

impl Ucd {
    /// Returns `version`'s files, or `None` where they are absent, as in a
    /// packaged crate.
    pub(crate) fn new(version: &str) -> Option<Self> {
        let dir = version_dir(version);
        let Ok(text) = fs::read_to_string(dir.join("PropertyValueAliases.txt")) else {
            eprintln!("{} is absent: the version check is skipped", dir.display());
            return None;
        };
        let mut aliases = HashMap::new();
        for fields in lines(&text) {
            if fields[0] == "ccc" {
                continue;
            }
            for name in &fields[1..] {
                aliases.insert((loose(fields[0]), loose(name)), fields[1].to_string());
            }
        }
        Some(Self { dir, aliases })
    }

    fn text(&self, file: &str) -> String {
        fs::read_to_string(self.dir.join(file)).expect("a pinned file")
    }

    fn short(&self, property: &str, value: &str) -> String {
        match self.aliases.get(&(loose(property), loose(value))) {
            Some(short) => short.clone(),
            None => value.to_string(),
        }
    }

    /// Returns each code point's value of an enumerated property, by short
    /// alias: the last field of the lines of `file` that give it (those
    /// whose second field is `named`, where given), with the `@missing`
    /// lines' defaults and then `default`.
    pub(crate) fn map(
        &self,
        file: &str,
        property: &str,
        default: &str,
        named: Option<&str>,
    ) -> Vec<String> {
        let text = self.text(file);
        let mut values = vec![self.short(property, default); CODE_POINTS];
        let wanted = |fields: &[&str]| match named {
            Some(name) => fields.len() == 3 && fields[1] == name,
            None => fields.len() == 2,
        };
        let missing = text
            .lines()
            .filter_map(|line| line.strip_prefix("# @missing:"))
            .map(|line| line.split(';').map(str::trim).collect::<Vec<_>>());
        for fields in missing.chain(lines(&text)) {
            if wanted(&fields) {
                let value = self.short(property, fields[fields.len() - 1]);
                for cp in range(fields[0]) {
                    values[cp].clone_from(&value);
                }
            }
        }
        values
    }

    /// Returns whether each code point has the binary property `property`,
    /// from the lines of `file` that name it.
    pub(crate) fn set(&self, file: &str, property: &str) -> Vec<bool> {
        let mut values = vec![false; CODE_POINTS];
        for fields in lines(&self.text(file)) {
            if fields.len() == 2 && fields[1] == property {
                for cp in range(fields[0]) {
                    values[cp] = true;
                }
            }
        }
        values
    }

    /// Returns each code point's Script_Extensions by short alias, its
    /// Script alone where ScriptExtensions.txt lists none.
    pub(crate) fn script_extensions(&self) -> Vec<Vec<String>> {
        let scripts = self.map("Scripts.txt", "sc", "Unknown", None);
        let mut values: Vec<Vec<String>> = scripts.into_iter().map(|script| vec![script]).collect();
        for fields in lines(&self.text("ScriptExtensions.txt")) {
            let listed: Vec<String> = fields[1]
                .split_whitespace()
                .map(|name| self.short("sc", name))
                .collect();
            for cp in range(fields[0]) {
                values[cp].clone_from(&listed);
            }
        }
        values
    }

    /// Returns each bracket's pair and type, `o` or `c`, from
    /// BidiBrackets.txt.
    pub(crate) fn brackets(&self) -> BTreeMap<usize, (usize, String)> {
        lines(&self.text("BidiBrackets.txt"))
            .map(|fields| {
                let pair = *range(fields[1]).start();
                (*range(fields[0]).start(), (pair, fields[2].to_string()))
            })
            .collect()
    }

    #[cfg(feature = "case")]
    /// Returns each code point's UnicodeData.txt fields that case mapping
    /// and Greek uppercasing read: General_Category, the combining class,
    /// the decomposition and the three simple mappings, joined, and
    /// SpecialCasing.txt's unconditional mappings after them.
    pub(crate) fn case_fields(&self) -> Vec<String> {
        let mut values = vec![String::new(); CODE_POINTS];
        let mut first = None;
        for line in self.text("UnicodeData.txt").lines() {
            let fields: Vec<&str> = line.split(';').collect();
            let cp = *range(fields[0]).start();
            let value = std::format!(
                "{};{};{};{};{};{}",
                fields[2],
                fields[3],
                fields[5],
                fields[12],
                fields[13],
                fields[14]
            );
            if fields[1].ends_with(", First>") {
                first = Some(cp);
            } else if fields[1].ends_with(", Last>") {
                let start = first.take().expect("a First> line");
                for slot in &mut values[start..=cp] {
                    slot.clone_from(&value);
                }
                continue;
            }
            values[cp] = value;
        }
        for fields in lines(&self.text("SpecialCasing.txt")) {
            let conditional = fields.get(4).is_some_and(|field| !field.is_empty());
            if !conditional {
                let cp = *range(fields[0]).start();
                values[cp] =
                    std::format!("{};{};{};{}", values[cp], fields[1], fields[2], fields[3]);
            }
        }
        values
    }
}

/// One version's Grapheme_Cluster_Break, Indic_Conjunct_Break and
/// Extended_Pictographic, read from its pinned UCD files.
pub(crate) struct GraphemeVersion(Vec<GraphemeProps>);

impl GraphemeVersion {
    /// Returns `version`'s properties, or `None` where its files are absent.
    pub(crate) fn new(version: &str) -> Option<Self> {
        let ucd = Ucd::new(version)?;
        let classes = ucd.map("GraphemeBreakProperty.txt", "GCB", "Other", None);
        let conjuncts = ucd.map("DerivedCoreProperties.txt", "InCB", "None", Some("InCB"));
        let pictographic = ucd.set("emoji-data.txt", "Extended_Pictographic");
        let props = classes
            .iter()
            .zip(&conjuncts)
            .zip(&pictographic)
            .map(|((class, conjunct), &pictographic)| {
                let class = match class.as_str() {
                    "XX" => GraphemeClusterBreak::Other,
                    "CN" => GraphemeClusterBreak::Control,
                    "CR" => GraphemeClusterBreak::Cr,
                    "EX" => GraphemeClusterBreak::Extend,
                    "L" => GraphemeClusterBreak::L,
                    "LF" => GraphemeClusterBreak::Lf,
                    "LV" => GraphemeClusterBreak::Lv,
                    "LVT" => GraphemeClusterBreak::Lvt,
                    "T" => GraphemeClusterBreak::T,
                    "V" => GraphemeClusterBreak::V,
                    "SM" => GraphemeClusterBreak::SpacingMark,
                    "PP" => GraphemeClusterBreak::Prepend,
                    "RI" => GraphemeClusterBreak::RegionalIndicator,
                    "ZWJ" => GraphemeClusterBreak::Zwj,
                    other => panic!("Grapheme_Cluster_Break {other}"),
                };
                // Indic_Conjunct_Break in bits 4 and 5, ICU4C's numbering.
                let conjunct = match conjunct.as_str() {
                    "None" => 0,
                    "Consonant" => 0x10,
                    "Extend" => 0x20,
                    "Linker" => 0x30,
                    other => panic!("Indic_Conjunct_Break {other}"),
                };
                let pictographic = if pictographic { 0x40 } else { 0 };
                GraphemeProps::from_bits(class as u8 | conjunct | pictographic)
            })
            .collect();
        Some(Self(props))
    }

    /// Returns the properties of `ch`.
    pub(crate) fn get(&self, ch: char) -> GraphemeProps {
        self.0[ch as usize]
    }

    /// Returns the properties of every code point, kept for the test run.
    pub(crate) fn leak(self) -> &'static [GraphemeProps] {
        std::boxed::Box::leak(self.0.into_boxed_slice())
    }
}

/// One version's word bytes, as the word segmenter reads them: the word
/// class, Extended_Pictographic and the dictionary run, read from its pinned
/// UCD files.
pub(crate) struct WordVersion(Vec<u8>);

impl WordVersion {
    /// Returns `version`'s word bytes, or `None` where its files are absent.
    pub(crate) fn new(version: &str) -> Option<Self> {
        let ucd = Ucd::new(version)?;
        let classes = ucd.map("WordBreakProperty.txt", "WB", "Other", None);
        let pictographic = ucd.set("emoji-data.txt", "Extended_Pictographic");
        let line_break = ucd.map("LineBreak.txt", "lb", "XX", None);
        let scripts = ucd.map("Scripts.txt", "sc", "Unknown", None);
        let bytes = (0..CODE_POINTS)
            .map(|cp| {
                let chinese_japanese = matches!(scripts[cp].as_str(), "Hani" | "Hira")
                    || (classes[cp] == "KA" && !(0xFF66..=0xFF9F).contains(&cp));
                let class: u8 = match classes[cp].as_str() {
                    "MB" if cp == 0x2E => 19,
                    "LE" if chinese_japanese => 20,
                    "XX" => 0,
                    "CR" => 1,
                    "LF" => 2,
                    "NL" => 3,
                    "Extend" => 4,
                    "ZWJ" => 5,
                    "RI" => 6,
                    "FO" => 7,
                    "KA" => 8,
                    "HL" => 9,
                    "LE" => 10,
                    "SQ" => 11,
                    "DQ" => 12,
                    "MB" => 13,
                    "ML" => 14,
                    "MN" => 15,
                    "NU" => 16,
                    "EX" => 17,
                    "WSegSpace" => 18,
                    other => panic!("Word_Break {other}"),
                };
                let run = if line_break[cp] == "SA" {
                    0x40
                } else if chinese_japanese {
                    0x80
                } else {
                    0
                };
                class | if pictographic[cp] { 0x20 } else { 0 } | run
            })
            .collect();
        Some(Self(bytes))
    }

    /// Returns the word byte of `ch`.
    pub(crate) fn get(&self, ch: char) -> u8 {
        self.0[ch as usize]
    }

    /// Returns the word bytes of every code point, kept for the test run.
    pub(crate) fn leak(self) -> &'static [u8] {
        std::boxed::Box::leak(self.0.into_boxed_slice())
    }
}

/// The two versions' files.
pub(crate) struct Versions {
    /// ICU's version.
    pub(crate) icu: Ucd,
    /// The tables' version.
    pub(crate) tables: Ucd,
}

impl Versions {
    /// Returns both versions' files, or `None` where they are absent.
    pub(crate) fn new() -> Option<Self> {
        Some(Self {
            icu: Ucd::new(ICU_VERSION)?,
            tables: Ucd::new(TABLES_VERSION)?,
        })
    }

    /// Returns the code points whose value of an enumerated property (as
    /// [`Ucd::map`] reads it) differs between the versions, with ICU's
    /// version's value and then the tables'.
    pub(crate) fn map_changes(
        &self,
        file: &str,
        property: &str,
        default: &str,
        named: Option<&str>,
    ) -> BTreeMap<u32, (String, String)> {
        let old = self.icu.map(file, property, default, named);
        let new = self.tables.map(file, property, default, named);
        changes(old, new)
    }

    /// Returns the code points whose binary property differs between the
    /// versions, with whether each has it in ICU's version and then in the
    /// tables'.
    pub(crate) fn set_changes(&self, file: &str, property: &str) -> BTreeMap<u32, (bool, bool)> {
        changes(
            self.icu.set(file, property),
            self.tables.set(file, property),
        )
    }
}

/// Returns the code points, surrogates aside, whose values differ between
/// `old` and `new`, with both values.
pub(crate) fn changes<T: PartialEq>(old: Vec<T>, new: Vec<T>) -> BTreeMap<u32, (T, T)> {
    (0u32..)
        .zip(old.into_iter().zip(new))
        .filter(|(cp, (old, new))| old != new && char::from_u32(*cp).is_some())
        .collect()
}

#[cfg(feature = "case")]
/// Returns the full mappings of `cp` alone in the root language, lowercase,
/// uppercase and titlecase, from its [`Ucd::case_fields`].
pub(crate) fn root_mappings(cp: u32, fields: &str) -> [String; 3] {
    let fields: Vec<&str> = fields.split(';').collect();
    let one = |field: &str| -> Option<String> {
        (!field.is_empty()).then(|| {
            field
                .split_whitespace()
                .filter_map(|hex| char::from_u32(u32::from_str_radix(hex, 16).ok()?))
                .collect()
        })
    };
    let itself = char::from_u32(cp).map(String::from).unwrap_or_default();
    let upper = one(fields.get(3).copied().unwrap_or_default()).unwrap_or_else(|| itself.clone());
    let lower = one(fields.get(4).copied().unwrap_or_default()).unwrap_or_else(|| itself.clone());
    let title = one(fields.get(5).copied().unwrap_or_default()).unwrap_or_else(|| upper.clone());
    match fields.get(6..9) {
        // SpecialCasing.txt's order: lowercase, titlecase, uppercase.
        Some([special_lower, special_title, special_upper]) => [
            one(special_lower).unwrap_or_default(),
            one(special_upper).unwrap_or_default(),
            one(special_title).unwrap_or_default(),
        ],
        _ => [lower, upper, title],
    }
}

/// Returns the five dictionaries the generator writes, as the
/// `dictionaries` feature compiles them in.
pub(crate) fn dictionaries() -> crate::Dictionaries {
    let dictionary = |bytes: &'static [u8]| crate::Dictionary::new(bytes).expect("a valid payload");
    crate::Dictionaries::new()
        .with_thai(dictionary(include_bytes!("../dictionary/data/thai.dafsa")))
        .and_then(|set| set.with_lao(dictionary(include_bytes!("../dictionary/data/lao.dafsa"))))
        .and_then(|set| {
            set.with_khmer(dictionary(include_bytes!("../dictionary/data/khmer.dafsa")))
        })
        .and_then(|set| {
            set.with_myanmar(dictionary(include_bytes!(
                "../dictionary/data/myanmar.dafsa"
            )))
        })
        .and_then(|set| {
            set.with_chinese_japanese(dictionary(include_bytes!("../dictionary/data/cj.dafsa")))
        })
        .expect("each payload in its slot")
}
