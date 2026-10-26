//! The pinned Unicode Character Database files, read and checked.
//!
//! Each file is listed in `manifest.txt` with the URL it was downloaded from
//! and its SHA-256. [`Ucd::new`] refuses a version whose files are missing,
//! unlisted or changed, so the generated tables always come from the files
//! the manifest names.
//!
//! The parsing follows UAX #44, section 4.2: fields split at `;`, comments
//! start at `#`, a code point range is `XXXX..YYYY`, and a `# @missing:` line
//! gives the value of every code point in its range that no data line lists.
//! Later `@missing` lines override earlier ones. Property values are matched
//! loosely (UAX44-LM3) against PropertyValueAliases.txt and stored by their
//! short alias.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::pinned::read_pinned;

/// The number of code points.
pub(crate) const CODE_POINTS: usize = 0x11_0000;

/// One version's checked files.
pub(crate) struct Ucd {
    dir: PathBuf,
    files: BTreeMap<String, String>,
    aliases: BTreeMap<(String, String), String>,
}

/// An enumerated property: each code point's value, by its short alias.
pub(crate) struct PropertyMap {
    names: Vec<String>,
    values: Vec<u8>,
}

impl PropertyMap {
    fn new(default: &str) -> Self {
        Self {
            names: vec![default.to_owned()],
            values: vec![0; CODE_POINTS],
        }
    }

    fn set(&mut self, start: u32, end: u32, value: &str) {
        let index = match self.names.iter().position(|name| name == value) {
            Some(index) => index,
            None => {
                self.names.push(value.to_owned());
                self.names.len() - 1
            }
        };
        let index = u8::try_from(index).expect("fewer than 256 values");
        self.values[start as usize..=end as usize].fill(index);
    }

    /// Returns the short alias of the value of `cp`.
    pub(crate) fn get(&self, cp: u32) -> &str {
        &self.names[usize::from(self.values[cp as usize])]
    }
}

/// A binary property: whether each code point has it.
pub(crate) struct PropertySet(Vec<bool>);

impl PropertySet {
    /// Returns whether `cp` has the property.
    pub(crate) fn contains(&self, cp: u32) -> bool {
        self.0[cp as usize]
    }

    /// Returns the property's ranges, adjacent ranges merged.
    pub(crate) fn ranges(&self) -> Vec<(u32, u32)> {
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for cp in (0..CODE_POINTS as u32).filter(|&cp| self.contains(cp)) {
            match ranges.last_mut() {
                Some(last) if last.1 + 1 == cp => last.1 = cp,
                _ => ranges.push((cp, cp)),
            }
        }
        ranges
    }
}

/// One line of UnicodeData.txt, for each code point it covers.
#[derive(Clone, Default)]
pub(crate) struct UnicodeData {
    pub(crate) general_category: String,
    pub(crate) combining_class: u8,
    /// The canonical decomposition, empty where there is none or it is a
    /// compatibility one.
    pub(crate) decomposition: Vec<u32>,
    pub(crate) upper: Option<u32>,
    pub(crate) lower: Option<u32>,
    pub(crate) title: Option<u32>,
}

/// An unconditional entry of SpecialCasing.txt: the full mappings.
pub(crate) struct SpecialCasing {
    pub(crate) lower: Vec<u32>,
    pub(crate) title: Vec<u32>,
    pub(crate) upper: Vec<u32>,
}

/// A data line: its code point range and its fields after the first.
struct Line<'a> {
    start: u32,
    end: u32,
    fields: Vec<&'a str>,
}

/// Parses `XXXX` or `XXXX..YYYY`.
fn range(text: &str) -> (u32, u32) {
    let text = text.trim();
    let parse =
        |hex: &str| u32::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("bad code point {hex}"));
    match text.split_once("..") {
        Some((start, end)) => (parse(start), parse(end)),
        None => (parse(text), parse(text)),
    }
}

/// Returns the data lines of `text`, comments and blank lines dropped.
fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter_map(|line| {
        let data = line.split('#').next().unwrap_or_default().trim();
        if data.is_empty() {
            return None;
        }
        let mut fields = data.split(';').map(str::trim);
        let (start, end) = range(fields.next()?);
        Some(Line {
            start,
            end,
            fields: fields.collect(),
        })
    })
}

/// Returns the `# @missing:` lines of `text`, in order.
fn missing(text: &str) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter_map(|line| {
        let data = line.strip_prefix("# @missing:")?.trim();
        let mut fields = data.split(';').map(str::trim);
        let (start, end) = range(fields.next()?);
        Some(Line {
            start,
            end,
            fields: fields.collect(),
        })
    })
}

/// Returns `name` in UAX44-LM3's loose form: no case, spaces, hyphens or
/// underscores, and no leading "is".
fn loose(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '-' | '_'))
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match name.strip_prefix("is") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => name,
    }
}

impl Ucd {
    /// Reads every file of `version` from `data`, checking each against the
    /// manifest.
    ///
    /// Returns an error where a file is missing, unlisted or changed.
    pub(crate) fn new(data: &Path, version: &str) -> Result<Self, String> {
        let dir = data.join(version);
        let files = read_pinned(data, version)?
            .into_iter()
            .map(|(name, bytes)| {
                let text = String::from_utf8(bytes)
                    .unwrap_or_else(|_| panic!("{version}/{name} is not UTF-8"));
                (name, text)
            })
            .collect();
        let mut ucd = Self {
            dir,
            files,
            aliases: BTreeMap::new(),
        };
        let mut aliases = BTreeMap::new();
        for line in ucd.text("PropertyValueAliases.txt").lines() {
            let data = line.split('#').next().unwrap_or_default().trim();
            if data.is_empty() {
                continue;
            }
            let fields: Vec<&str> = data.split(';').map(str::trim).collect();
            // Canonical_Combining_Class is read as a number from
            // UnicodeData.txt, and its lines put the number first.
            if fields[0] == "ccc" {
                continue;
            }
            let property = loose(fields[0]);
            for name in &fields[1..] {
                aliases.insert((property.clone(), loose(name)), fields[1].to_owned());
            }
        }
        ucd.aliases = aliases;
        Ok(ucd)
    }

    fn text(&self, name: &str) -> &str {
        self.files
            .get(name)
            .unwrap_or_else(|| panic!("{} has no {name}", self.dir.display()))
    }

    /// Returns the short alias of `value` for `property`.
    fn short(&self, property: &str, value: &str) -> String {
        self.aliases
            .get(&(loose(property), loose(value)))
            .unwrap_or_else(|| panic!("{property} has no value {value:?}"))
            .clone()
    }

    /// Reads an enumerated property whose values are the second field of
    /// `file`, with `default` where neither a data line nor an `@missing`
    /// line gives one.
    pub(crate) fn map(&self, file: &str, property: &str, default: &str) -> PropertyMap {
        let text = self.text(file);
        let mut map = PropertyMap::new(&self.short(property, default));
        for line in missing(text) {
            let [value] = line.fields[..] else {
                panic!("{file}: an @missing line with other than one value");
            };
            map.set(line.start, line.end, &self.short(property, value));
        }
        for line in lines(text) {
            map.set(line.start, line.end, &self.short(property, line.fields[0]));
        }
        map
    }

    /// Reads an enumerated property from the lines of `file` whose second
    /// field names it, as DerivedCoreProperties.txt gives InCB.
    pub(crate) fn named_map(&self, file: &str, property: &str, default: &str) -> PropertyMap {
        let text = self.text(file);
        let mut map = PropertyMap::new(&self.short(property, default));
        for line in missing(text) {
            if let [name, value] = line.fields[..]
                && name == property
            {
                map.set(line.start, line.end, &self.short(property, value));
            }
        }
        for line in lines(text) {
            if let [name, value] = line.fields[..]
                && name == property
            {
                map.set(line.start, line.end, &self.short(property, value));
            }
        }
        map
    }

    /// Reads a binary property from the lines of `file` that name it.
    pub(crate) fn set(&self, file: &str, property: &str) -> PropertySet {
        let mut set = vec![false; CODE_POINTS];
        let mut found = false;
        for line in lines(self.text(file)) {
            if line.fields.len() == 1 && line.fields[0] == property {
                set[line.start as usize..=line.end as usize].fill(true);
                found = true;
            }
        }
        assert!(found, "{file} lists no {property}");
        PropertySet(set)
    }

    /// Reads Script, as short aliases, with Unknown as the default.
    pub(crate) fn scripts(&self) -> PropertyMap {
        self.map("Scripts.txt", "sc", "Unknown")
    }

    /// Reads Script_Extensions where ScriptExtensions.txt lists them: each
    /// code point's scripts by short alias, in the file's order.
    pub(crate) fn script_extensions(&self) -> BTreeMap<u32, Vec<String>> {
        let mut extensions = BTreeMap::new();
        for line in lines(self.text("ScriptExtensions.txt")) {
            let scripts: Vec<String> = line.fields[0]
                .split_whitespace()
                .map(|name| self.short("sc", name))
                .collect();
            for cp in line.start..=line.end {
                extensions.insert(cp, scripts.clone());
            }
        }
        extensions
    }

    /// Reads BidiBrackets.txt: each bracket's pair and whether it opens.
    pub(crate) fn brackets(&self) -> BTreeMap<u32, (u32, bool)> {
        let mut brackets = BTreeMap::new();
        for line in lines(self.text("BidiBrackets.txt")) {
            let [paired, kind] = line.fields[..] else {
                panic!("BidiBrackets.txt: a line without two fields");
            };
            let (paired, _) = range(paired);
            let opens = match kind {
                "o" => true,
                "c" => false,
                other => panic!("BidiBrackets.txt: bracket type {other:?}"),
            };
            brackets.insert(line.start, (paired, opens));
        }
        brackets
    }

    /// Reads UnicodeData.txt, filling the ranges its `First>` and `Last>`
    /// lines give, and the default for every other code point.
    pub(crate) fn unicode_data(&self) -> Vec<UnicodeData> {
        let mut data = vec![
            UnicodeData {
                general_category: "Cn".to_owned(),
                ..UnicodeData::default()
            };
            CODE_POINTS
        ];
        let mut first = None;
        for line in self.text("UnicodeData.txt").lines() {
            let fields: Vec<&str> = line.split(';').collect();
            assert_eq!(fields.len(), 15, "UnicodeData.txt: {line}");
            let cp = range(fields[0]).0;
            let optional = |field: &str| (!field.is_empty()).then(|| range(field).0);
            let decomposition = if fields[5].is_empty() || fields[5].starts_with('<') {
                Vec::new()
            } else {
                fields[5].split_whitespace().map(|cp| range(cp).0).collect()
            };
            let entry = UnicodeData {
                general_category: fields[2].to_owned(),
                combining_class: fields[3].parse().expect("a combining class is a number"),
                decomposition,
                upper: optional(fields[12]),
                lower: optional(fields[13]),
                title: optional(fields[14]),
            };
            if fields[1].ends_with(", First>") {
                first = Some(cp);
                data[cp as usize] = entry;
            } else if fields[1].ends_with(", Last>") {
                let start = first.take().expect("a Last> line follows its First>");
                data[start as usize..=cp as usize].fill(entry);
            } else {
                data[cp as usize] = entry;
            }
        }
        data
    }

    /// Reads SpecialCasing.txt's unconditional entries.
    pub(crate) fn special_casing(&self) -> BTreeMap<u32, SpecialCasing> {
        let mut special = BTreeMap::new();
        for line in lines(self.text("SpecialCasing.txt")) {
            let conditions = line.fields.get(3).copied().unwrap_or_default();
            if !conditions.is_empty() {
                continue;
            }
            let mapping = |field: &str| -> Vec<u32> {
                field.split_whitespace().map(|cp| range(cp).0).collect()
            };
            special.insert(
                line.start,
                SpecialCasing {
                    lower: mapping(line.fields[0]),
                    title: mapping(line.fields[1]),
                    upper: mapping(line.fields[2]),
                },
            );
        }
        special
    }
}
