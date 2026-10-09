//! The encapsulation, naming and comment guard.
//!
//! Reads the crate's own sources as text and fails on:
//! - a field opened past the crate, a function whose name has a blocked
//!   word ([`BLOCKED_WORDS`]), a `usize` range into a table, or a constructor
//!   with a name of its own;
//! - a door that answers by cluster or by offset, called outside the files
//!   allowed to call it;
//! - the caller's style named past the door;
//! - history or a doc reference in a comment.
//!
//! Frozen stage data may show its fields to the crate, `pub(crate)` or
//! `pub(super)`, since a later stage holds only `&` of it. A bare `pub`
//! field on a type the crate keeps to itself is a field no reader should
//! see, and fails.
//!
//! Each check is a heuristic over the text, tuned to find nothing in code
//! that keeps the rules. Each real exception is on an allowlist below, one
//! line with its reason, so that breaking a rule is a decision someone
//! writes down. An entry that no longer excuses anything fails too, so the
//! lists only shrink.
//!
//! Test code is exempt from all but the comment check: `tests.rs` files,
//! `tests/` directories, and `#[cfg(test)]` modules inside a source file.
//!
//! The doors that answer by cluster or by offset stay in the files allowed
//! to call them. A walk carries the runs it is inside, and a search happens
//! only where a random-access read starts. Each file calling one is on an
//! allowlist with its reason: an entry, where such a read starts and seeks
//! once, or a door's own body. A new caller fails until it is an entry or
//! walks.
//!
//! The caller's style stays at the door: `ComputedStyle`, the content's
//! tables by `.styles.`, and the facts' constructors are named only in
//! `stages/content/` and `style/`, where the door lowers each style into the facts
//! every later stage reads. A file elsewhere naming one is on an allowlist
//! with its reason.
//!
//! The comment check reads every `.rs` file under `src` and `tests`, test
//! code included. Its patterns are [`HISTORY`].
//!
//! What it cannot see is left to review: a method that only forwards to a
//! free function, a method and a free function sharing a name, an enum
//! converted to another by an inline `match`, and a search over another
//! stage's table written inline.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// An allowlist entry: the file, the item it excuses, and the reason.
type Entry = (&'static str, &'static str, &'static str);

/// Crate-private structs whose fields may be `pub`, past the crate, by file
/// and struct, each with its reason.
const ALLOWED_FIELDS: &[Entry] = &[];

/// The words a function's name may not have as one of its `_`-separated
/// parts.
/// A name that is the word `at` alone is a lookup on its own collection
/// (`Clusters::at`), which keeps the rule.
const BLOCKED_WORDS: &[&str] = &["holding", "of", "in", "for", "own", "at"];

/// Functions whose names have a blocked word that Rust's own naming backs,
/// by file and name, each with its reason.
const ALLOWED_NAMES: &[Entry] = &[];

/// Lines holding a `Range<usize>` outside the data module and `unit.rs`, by
/// file and a piece of the line, each with its reason: a byte range into a
/// string, which indexes no table.
#[rustfmt::skip]
const ALLOWED_USIZE_RANGES: &[Entry] = &[
    ("src/layout/line.rs", "pub fn text_range", "public API: a line's text as a byte range into the caller's `Layout::text`"),
    ("src/layout/run.rs", "pub fn text_range", "public API: an item's or a cluster's text as a byte range into the caller's `Layout::text`"),
    ("src/layout/boxes.rs", "pub fn text_range", "public API: an atomic inline's U+FFFC as a byte range into the caller's `Layout::text`"),
    ("src/selection/mod.rs", "pub fn range", "public API: a selection's text as a byte range into the caller's `Layout::text`"),
    ("src/layout/mod.rs", "pub fn selection_rects", "public API: the text a selection paints, a byte range into the caller's `Layout::text`"),
    ("src/layout/mod.rs", "        range: Range<usize>,", "public API: `selected_text`'s range, the text a selection copies, a byte range into the caller's `Layout::text`, its parameter on a line of its own"),
    ("src/stages/content/lists.rs", "fn range(self)", "a byte range into the string the interned family lists keep their names in, to slice a `str` with"),
    ("src/unicode/bidi.rs", "fn range(&self)", "the bidi algorithm: a level run's paragraph positions"),
];

/// Constructors not named `new`, `from_*` or `with_*`, by file and name,
/// each with its reason.
#[rustfmt::skip]
const ALLOWED_CONSTRUCTORS: &[Entry] = &[
    ("src/config.rs", "chrome_windows", "public API: a preset, named for the browser and platform whose behavior it is"),
    ("src/config.rs", "chrome_mac", "public API: a preset, named for the browser and platform whose behavior it is"),
    ("src/config.rs", "chrome_linux", "public API: a preset, named for the browser and platform whose behavior it is"),
    ("src/config.rs", "platform", "public API: the preset of the platform the crate is built for"),
    ("src/config.rs", "spec", "public API: the configuration named for the CSS specifications it follows"),
    ("src/style/mod.rs", "initial", "public API: CSS's own name for every property at its initial value"),
    ("src/style/edges.rs", "all", "public API: one value on every side, as a CSS shorthand given one value sets them"),
];

/// The doors that answer by cluster or by offset, which a walk carrying its
/// runs does not call: each as it reads in code with its white
/// space taken out, a call and not a definition.
const DOORS: &[&str] = &[
    // The run, cluster, line, paragraph or column base containing a
    // position, by halving or by rank.
    ".containing(",
    "containing_offset(",
    "run_containing(",
    "clusters.at(",
    "lines.at(",
    "place::line(",
    "base_containing(",
    // The clusters or edges of what contains a position.
    "clusters_containing(",
    "line_edges_touching(",
    "first_reaching(",
    "empty_items(",
    // A walk's seek, and a place in the flow sought.
    "cursor_containing(",
    "Slot::new(",
    // The halvings themselves.
    "first_from(",
    "last_where(",
    "partition_point(",
    // A cursor over runs moved by a cluster, which a `RunCursor` replaces.
    "runs.advance(",
    "paragraphs.advance(",
];

/// The files that may call a door of [`DOORS`], by file and door, each with
/// its reason: an entry, where a random-access read starts and seeks once;
/// a door's or a primitive's own body; or a search over a table that is not
/// of runs.
#[rustfmt::skip]
const ALLOWED_DOORS: &[Entry] = &[
    ("src/stages/lines/ruby.rs", "partition_point(", "retained continuation rows are keyed by line, not runs; rewind finds the first discarded line"),
    ("src/stages/lines/ruby.rs", "lines.at(", "entry: the lines a caret's or a selection's ruby pieces can be on, from the clusters at its ends, seeking once each"),
    ("src/stages/lines/height.rs", "cursor_containing(", "entry: a line's text that fallback fonts widen across its edges seeks its shaping run once a line, then carries the cursor"),
    // The doors' and the primitives' own bodies.
    ("src/data/runs.rs", ".containing(", "body: `Runs::cursor_containing` seeks by `Runs::containing`"),
    ("src/data/runs.rs", "last_where(", "bodies: `Runs::containing` and `Runs::last_where`, the halving"),
    ("src/data/runs.rs", "partition_point(", "body: `Runs::first_from`, the halving"),
    ("src/data/sorted.rs", "partition_point(", "body: `find_sorted`, the one search by key"),
    ("src/data/table.rs", "partition_point(", "body: `Table::last_where`, the halving"),
    ("src/stages/analysis/paragraphs.rs", ".containing(", "bodies: `Paragraphs::containing`, by `Runs::containing`, and `Paragraphs::clusters_containing`, by `containing`"),
    ("src/stages/analysis/paragraphs.rs", "cursor_containing(", "body: `Paragraphs::cursor_containing`, the walk's seek"),
    ("src/stages/analysis/items.rs", "first_from(", "bodies: `ItemClusters::empty_items` and `cursor_containing`"),
    ("src/stages/analysis/clusters.rs", "partition_point(", "body: `Clusters::at`, a caller's offset to its cluster"),
    ("src/stages/shape/mod.rs", ".containing(", "bodies: `run_containing`, `clusters_containing` and `cursor_containing`, by the rank of the start bits"),
    ("src/stages/lines/mod.rs", "partition_point(", "bodies: `Lines::first_reaching` and `Lines::at`"),
    ("src/stages/measure/ruby_columns.rs", "last_where(", "body: `RubyColumns::base_containing`"),
    ("src/stages/measure/ruby_columns.rs", "partition_point(", "body: `RubyColumns::empty_room`, the room of the empty nested columns at a boundary"),
    ("src/stages/measure/generated.rs", "partition_point(", "body: `GeneratedTexts::find`, keyed by text facts, not a table of runs"),
    ("src/stages/measure/pen.rs", "cursor_containing(", "body: `LineStages::cluster_advance_near`, the item at a cluster's start where boxes have edges, sought once a walk over hanging or cut clusters"),
    ("src/stages/segments.rs", "cursor_containing(", "bodies: `Segments::new`, the walk's one seek, and `Slot::new`, the one place deciding a boundary's item"),
    ("src/stages/segments.rs", ".containing(", "body: `Slot::new`, the one place deciding a boundary's shaping run, by rank"),
    // Tables of another kind than runs of clusters.
    ("src/stages/content/map.rs", "first_from(", "the offset map: the caller's offset axis"),
    ("src/stages/content/map.rs", "last_where(", "the offset map: the caller's offset axis"),
    ("src/stages/content/map.rs", "partition_point(", "the offset map: the caller's offset axis, and the writer's trailing trim, once a build"),
    ("src/stages/content/first_line.rs", "partition_point(", "the first line's text map by content offset, read where a first line transforms text its own way: the content's offset axis, as the offset map's"),
    ("src/stages/fragments/boxes.rs", "partition_point(", "line layout's box parts, by their left: geometry"),
    ("src/stages/measure/items.rs", "partition_point(", "body: `KeptBoxes::first_from`, a walk's one seek among the kept boxes by node, not a table of runs"),
    ("src/path/curves.rs", "partition_point(", "text on a path: a segment of the path by arc length"),
    ("src/stages/measure/scan_em_boxes.rs", "cursor_containing(", "entry: the unshaped first-line tail of an item, sought once then walked in font order"),
    // Entries: a random-access read starts here, seeks once, and walks.
    ("src/selection/motion.rs", ".containing(", "entry: `paragraph_level` and `paragraph_boundary` seek the focus's paragraph from its cluster, once a motion"),
    ("src/selection/place.rs", "clusters.at(", "entry: `Spot::new`, a caller's offset to its cluster, the one search a public call makes from an offset; the rest steps from the spot by id"),
    ("src/selection/place.rs", "lines.at(", "entry: the line containing a position"),
    ("src/selection/place.rs", "empty_items(", "entry: the boxes at a position"),
    ("src/selection/place.rs", "first_reaching(", "entry: the line upstream of a position"),
    ("src/selection/rects.rs", "place::line(", "entry: the lines holding a selection's ends"),
    ("src/selection/visual.rs", "place::line(", "entry: the lines holding a character step's ends, which tell the step on the screen from the one in text order without placing either caret"),
    ("src/selection/words.rs", "clusters_containing(", "entry: the paragraph around a word boundary, kept while a motion stays in it"),
    ("src/layout/boxes.rs", "first_reaching(", "entry: an atomic inline's line, read alone"),
    ("src/layout/paint.rs", "first_from(", "entry: a line's paint starts its walk over the kept boxes at its first node, once a paint, where the line has no box item to start from"),
    ("src/layout/emphasis.rs", ".containing(", "entry: the font runs of a fragment whose item's fonts have no one em box, read alone, for its marks' em box"),
    ("src/stages/measure/ruby_columns.rs", "base_containing(", "entry: `outermost_annotation` seeks the base containing a marked item once, then follows the bounded parent chain"),
    ("src/layout/run.rs", "run_containing(", "entry: an annotation's text run's shaping run, which its item does not name, by rank, read alone"),
    ("src/stages/fragments/ruby.rs", "line_edges_touching(", "entry: an annotation line's reshaped edges, sought once a level rather than once a cluster"),
    ("src/stages/fragments/read.rs", "line_edges_touching(", "entry: a fragment's line's reshaped edges, read alone"),
    ("src/stages/lines/fit_reshape.rs", "run_containing(", "entry: `text-spacing-trim` at a line's start or end mark, from its shaping run's facts, by rank once a line edge"),
    ("src/stages/lines/fit_reshape.rs", ".containing(", "entry: the breaker's edge reshapes, once a line edge: the shaping run of a start window or an end piece, by rank, which the reshape takes with its neighbours, the run's id ± 1"),
    ("src/stages/measure/autospace.rs", "cursor_containing(", "entry: `gap_after`, a seam at a line's end, seeks the item after it where the characters leave a seam possible"),
    ("src/stages/measure/autospace.rs", ".containing(", "entry: `gap_after`'s run after a seam, by rank, where a paragraph's levels differ and the caller holds no run; the side before steps back from it"),
    ("src/stages/measure/autospace.rs", "Slot::new(", "entries: `room_before`, a seam where line layout draws its room, seeks the place after it once; `gap_after_near` seeks once a reshaped piece, then steps"),
    ("src/stages/measure/justify.rs", ".containing(", "entry: a line's opportunities, read from its start: the script-run cursor's one seek, where some text is combined or may be cursive"),
    ("src/stages/measure/justify.rs", "cursor_containing(", "entry: a line's opportunities, read from its start: the item cursor's one seek, where some style sets `text-justify`"),
    ("src/stages/lines/annotate.rs", "cursor_containing(", "a line's em boxes: the font runs of a text item with no one em box for all its text, sought once a line, and walked with the items"),
];

/// What only the door names: the caller's style, and the
/// content's tables reached as a style table's were. Each as it reads in
/// code with its white space taken out.
const STYLE_NAMES: &[&str] = &["ComputedStyle", ".styles."];

/// The facts' records, whose constructors only the door calls: a
/// call of `new`, or a literal, `Name {`.
const FACTS: &[&str] = &[
    "TextFacts",
    "ShapingFacts",
    "FontRequest",
    "BoxFacts",
    "BlockFacts",
];

/// The files outside `stages/content/` and `style/` that may name a pattern of
/// [`STYLE_NAMES`] or construct a record of [`FACTS`], by file and pattern
/// or record, each with its reason.
#[rustfmt::skip]
const ALLOWED_STYLES: &[Entry] = &[
    ("src/build.rs", "ComputedStyle", "public API: the builder's calls take the caller's style, which each hands to the door"),
    ("src/lib.rs", "ComputedStyle", "public API: the crate's re-export of the door's input"),
    ("src/stages/fonts/select.rs", "FontRequest", "`faces_alike`: a first-line request at its node's own size, compared and never kept"),
];

#[test]
fn fields_names_and_indexes_keep_the_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    files.sort();

    let mut guard = Guard::default();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        match fs::read_to_string(path) {
            Ok(text) => guard.check(&file, &Code::new(&text)),
            Err(error) => guard.fail(&file, 0, &format!("unreadable: {error}"), ""),
        }
    }
    guard.check_stale_entries();
    assert!(
        guard.report.is_empty(),
        "\nThe encapsulation and naming rules are broken:\n\n{}",
        guard.report
    );
}

/// The checks find what they look for, and nothing in code that keeps the
/// rules, comments, literals and test modules included: a guard whose
/// reading of the code had broken would find nothing, and pass everything.
#[test]
fn the_checks_find_what_they_look_for() {
    let source = r##"
pub(crate) struct Record {
    pub(crate) wide: u32,
    kept: u32,
    #[allow(dead_code)]
    pub inside: u32,
}
pub struct Public {
    pub open: u32,
}
struct Hidden(pub u32, pub(super) u32);
impl Record {
    pub(crate) fn columns_of(&self) -> Range<usize> {
        0..0
    }
    fn read(data: &[u8]) -> Option<Self> {
        None
    }
    fn new() -> Self {
        todo!()
    }
    fn from_bytes(data: &[u8]) -> Result<Self, ()> {
        todo!()
    }
    fn with_wide(self, wide: u32) -> Self {
        self
    }
    fn at(&self) -> Self {
        *self
    }
    fn item_at(&self, at: u32) -> Option<u32> {
        self.runs
            .run_containing(at)
            .or(self.base_containing(at))
    }
}
impl Default for Record {
    fn default() -> Self {
        todo!()
    }
}
// fn commented_of() -> Self
/* pub(crate) struct Commented { pub(crate) field: u32 } */
const TEXT: &str = "fn quoted_of() -> Range<usize> { '{' }";
const BRACE: char = '{';
fn reads(content: &Content) -> BoxFacts {
    let style: &ComputedStyle = todo!();
    BoxFacts { ..content.styles.get(id) }
}
fn text_in(at: u32) {}
fn stand_in_for() {}
fn inside_formatted_holdings() {}
#[cfg(test)]
mod tests {
    struct Fixture {
        pub open: u32,
    }
    fn fixture_of() {}
}
"##;
    let mut guard = Guard::default();
    guard.check("src/example.rs", &Code::new(source));
    let found: Vec<&str> = guard
        .report
        .lines()
        .filter(|line| !line.starts_with("    fix"))
        .collect();
    assert_eq!(
        found,
        [
            "src/example.rs:6: field `inside` of `Record` is `pub`",
            "src/example.rs:11: field `0` of `Hidden` is `pub`",
            "src/example.rs:13: `fn columns_of`",
            "src/example.rs:31: `fn item_at`",
            "src/example.rs:50: `fn text_in`",
            "src/example.rs:51: `fn stand_in_for`",
            "src/example.rs:13: `Range<usize>`",
            "src/example.rs:16: constructor `fn read`",
            "src/example.rs:33: a door answering by cluster or offset, `run_containing(`",
            "src/example.rs:34: a door answering by cluster or offset, `base_containing(`",
            "src/example.rs:47: a style or a fact past the door, `ComputedStyle`",
            "src/example.rs:48: a style or a fact past the door, `.styles.`",
            "src/example.rs:48: a style or a fact past the door, `BoxFacts`",
        ]
    );
}

/// Every `.rs` file under `dir` but test code.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                collect(&path, files);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            files.push(path);
        }
    }
}

/// What a comment may not say: history, which lives in git, and pointers
/// at the docs, which a comment restates in place instead.
#[rustfmt::skip]
const HISTORY: &[Banned] = &[
    Banned { text: "old crate", digit: false, word: false, ignore_case: false },
    Banned { text: "plan step", digit: false, word: false, ignore_case: false },
    Banned { text: "wave", digit: false, word: true, ignore_case: true },
    Banned { text: "design ", digit: true, word: false, ignore_case: false },
    Banned { text: "research ", digit: false, word: false, ignore_case: false },
    Banned { text: "research/", digit: false, word: false, ignore_case: false },
    Banned { text: "survey ", digit: true, word: false, ignore_case: false },
    Banned { text: "\u{a7}", digit: false, word: false, ignore_case: false },
    Banned { text: "hard-won", digit: false, word: false, ignore_case: false },
];

/// A pattern of [`HISTORY`]: its text, then a digit where `digit` says,
/// alone as a word where `word` says.
struct Banned {
    text: &'static str,
    digit: bool,
    word: bool,
    ignore_case: bool,
}

impl Banned {
    /// Whether `comment` says this.
    fn is_said_by(&self, comment: &str) -> bool {
        let comment = if self.ignore_case {
            comment.to_lowercase()
        } else {
            comment.to_owned()
        };
        comment.match_indices(self.text).any(|(at, _)| {
            let before = comment[..at].chars().next_back();
            let after = comment[at + self.text.len()..].chars().next();
            let word_char =
                |ch: Option<char>| ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
            (!self.digit || after.is_some_and(|ch| ch.is_ascii_digit()))
                && (!self.word || (!word_char(before) && !word_char(after)))
        })
    }
}

/// No comment anywhere in the crate, tests included, tells history or
/// points at the docs.
#[test]
fn comments_tell_no_history_and_cite_no_docs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_every(&root.join("src"), &mut files);
    collect_every(&root.join("tests"), &mut files);
    files.sort();

    let mut report = String::new();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        match fs::read_to_string(path) {
            Ok(text) => report.push_str(&history(&file, &text)),
            Err(error) => {
                let _ = writeln!(report, "{file}: unreadable: {error}");
            }
        }
    }
    assert!(
        report.is_empty(),
        "\nThese comments tell history or point at the docs; say what they \
         mean in place, or drop them:\n\n{report}"
    );
}

/// No file in the crate names its module's file by attribute: every module
/// is where Cargo and rustc look for it, `foo/mod.rs` or a single `foo.rs`.
#[test]
fn no_module_is_found_by_a_path_attribute() {
    // Spelled in two halves, so that this file does not name it.
    let attribute = concat!("#[", "path");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_every(root, &mut files);
    files.sort();

    let mut report = String::new();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(path) else {
            let _ = writeln!(report, "{file}: unreadable");
            continue;
        };
        for (at, line) in text.lines().enumerate() {
            if line.contains(attribute) {
                let _ = writeln!(report, "{file}:{}: {}", at + 1, line.trim());
            }
        }
    }
    assert!(
        report.is_empty(),
        "\nThese name a module's file by attribute; move the file to where \
         its `mod` finds it:\n\n{report}"
    );
}

/// No item in the crate is visible by path: an item is private,
/// `pub(super)`, `pub(crate)` or `pub`, so its module's `mod.rs` shows who
/// may reach it.
#[test]
fn no_item_is_visible_by_path() {
    // Spelled in two halves, so that this file does not name it.
    let visibility = concat!("pub(", "in ");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_every(&root.join("src"), &mut files);
    files.sort();

    let mut report = String::new();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(path) else {
            let _ = writeln!(report, "{file}: unreadable");
            continue;
        };
        for (at, line) in text.lines().enumerate() {
            if line.contains(visibility) {
                let _ = writeln!(report, "{file}:{}: {}", at + 1, line.trim());
            }
        }
    }
    assert!(
        report.is_empty(),
        "\nThese are visible by path; make them private, `pub(super)` or \
         `pub(crate)`, moving a shared item into its module's `mod.rs`:\n\n{report}"
    );
}

/// A module's children are private: its `mod.rs` re-exports what other
/// modules use. The stages are the exception, since code names a stage
/// through `crate::stages`.
#[test]
fn child_modules_are_private() {
    // Spelled in two halves, so that this file does not name them.
    let widened = [
        concat!("pub(crate) ", "mod "),
        concat!("pub(super) ", "mod "),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_every(&root.join("src"), &mut files);
    files.sort();

    let mut report = String::new();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if file == "src/stages/mod.rs" {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            let _ = writeln!(report, "{file}: unreadable");
            continue;
        };
        for (at, line) in text.lines().enumerate() {
            if widened
                .iter()
                .any(|start| line.trim_start().starts_with(start))
            {
                let _ = writeln!(report, "{file}:{}: {}", at + 1, line.trim());
            }
        }
    }
    assert!(
        report.is_empty(),
        "\nThese modules are wider than private; make them `mod`, and \
         re-export from the parent's `mod.rs` what other modules use:\n\n{report}"
    );
}

/// The comment check finds each pattern in a comment of every kind, and
/// nothing in code or in a literal.
#[test]
fn the_comment_check_finds_what_it_looks_for() {
    let source = r##"
// Kept from the old crate.
/// Added by plan step 4.
//! The Wave 2 renames.
fn a() {} // as design 03 says
/// See research/notes.md.
/// The research note says so.
/// Survey 06 is about fonts; survey 06 found it.
/// As section 5 says, not as \u{a7}5 does.
/// A hard-won lesson.
/// A wavelength, a microwave, wavy, a design choice, a survey of
/// 3 fonts, researching, a plan, a step.
fn wave(old_crate: u32) -> &'static str {
    let design = "the old crate's wave, design 03, research/ survey 06";
    let url = "https://plan step"; // fine
    '/'
}
"##
    .replace("\\u{a7}", "\u{a7}");
    assert_eq!(
        history("src/example.rs", &source),
        "src/example.rs:2: // Kept from the old crate.\n\
         src/example.rs:3: /// Added by plan step 4.\n\
         src/example.rs:4: //! The Wave 2 renames.\n\
         src/example.rs:5: // as design 03 says\n\
         src/example.rs:6: /// See research/notes.md.\n\
         src/example.rs:7: /// The research note says so.\n\
         src/example.rs:8: /// Survey 06 is about fonts; survey 06 found it.\n\
         src/example.rs:9: /// As section 5 says, not as \u{a7}5 does.\n\
         src/example.rs:10: /// A hard-won lesson.\n"
    );
}

/// Each comment in `text` that says a pattern of [`HISTORY`], one line
/// each, as `file:line: comment`.
fn history(file: &str, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut report = String::new();
    let mut line = 1;
    let mut counted = 0;
    scan(&chars, |span, from, to| {
        if span != Span::LineComment {
            return;
        }
        line += chars[counted..from]
            .iter()
            .filter(|&&ch| ch == '\n')
            .count();
        counted = from;
        let comment: String = chars[from..to].iter().collect();
        if HISTORY.iter().any(|banned| banned.is_said_by(&comment)) {
            let _ = writeln!(report, "{file}:{line}: {}", comment.trim_end());
        }
    });
    report
}

/// Every `.rs` file under `dir`, test code included.
fn collect_every(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_every(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

// --------------------------------------------------------------------------
// The checks
// --------------------------------------------------------------------------

#[derive(Default)]
struct Guard {
    report: String,
    used_fields: Vec<usize>,
    used_names: Vec<usize>,
    used_ranges: Vec<usize>,
    used_constructors: Vec<usize>,
    used_doors: Vec<usize>,
    used_styles: Vec<usize>,
}

impl Guard {
    fn check(&mut self, file: &str, code: &Code) {
        self.fields(file, code);
        self.blocked_names(file, code);
        self.usize_ranges(file, code);
        self.constructors(file, code);
        self.doors(file, code);
        self.styles(file, code);
    }

    /// A pattern of [`STYLE_NAMES`] named, or a record of [`FACTS`]
    /// constructed, outside `stages/content/` and `style/` and not on
    /// [`ALLOWED_STYLES`]: a stage after the door reading a style, or
    /// making a fact the door should have lowered.
    fn styles(&mut self, file: &str, code: &Code) {
        if file.starts_with("src/stages/content/") || file.starts_with("src/style/") {
            return;
        }
        let squeezed: String = code
            .lines
            .iter()
            .flat_map(|line| line.chars().filter(|c| !c.is_whitespace()))
            .collect();
        let mut found: Vec<(usize, &str)> = Vec::new();
        for &name in STYLE_NAMES {
            if squeezed.contains(name) {
                let line = code.lines.iter().position(|line| {
                    let line: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                    line.contains(name)
                });
                found.push((line.map_or(0, |at| at + 1), name));
            }
        }
        for (index, line) in code.lines.iter().enumerate() {
            for &record in FACTS {
                if constructs(line, record) {
                    found.push((index + 1, record));
                }
            }
        }
        for (line, name) in found {
            let allowed = ALLOWED_STYLES
                .iter()
                .position(|&(f, n, _)| f == file && n == name);
            if let Some(entry) = allowed {
                if !self.used_styles.contains(&entry) {
                    self.used_styles.push(entry);
                }
                continue;
            }
            self.fail(
                file,
                line,
                &format!("a style or a fact past the door, `{name}`"),
                "read the facts the door lowered (`content::Facts`, the node's ids), never \
                 the caller's style; lower a new fact at the door, in `stages/content/`; where this \
                 file must name it, add the file and name to ALLOWED_STYLES with the reason",
            );
        }
    }

    /// A call of a door of [`DOORS`] in a file not on [`ALLOWED_DOORS`] for
    /// it: a search by cluster or offset, where a walk should carry the run
    /// it asks for. Read over the code with its white space taken
    /// out, so a call whose receiver is on the line before is seen.
    fn doors(&mut self, file: &str, code: &Code) {
        let mut squeezed = Vec::new();
        for (index, line) in code.lines.iter().enumerate() {
            squeezed.extend(
                line.chars()
                    .filter(|c| !c.is_whitespace())
                    .map(|c| (c, index + 1)),
            );
        }
        let chars: Vec<char> = squeezed.iter().map(|&(c, _)| c).collect();
        for &door in DOORS {
            let needle: Vec<char> = door.chars().collect();
            let mut at = 0;
            while at + needle.len() <= chars.len() {
                if chars[at..at + needle.len()] != needle[..] {
                    at += 1;
                    continue;
                }
                let line = squeezed[at].1;
                at += needle.len();
                if defines(&code.lines[line - 1], door) {
                    continue;
                }
                let allowed = ALLOWED_DOORS
                    .iter()
                    .position(|&(f, d, _)| f == file && d == door);
                if let Some(entry) = allowed {
                    self.used_doors.push(entry);
                    continue;
                }
                self.fail(
                    file,
                    line,
                    &format!("a door answering by cluster or offset, `{door}`"),
                    "walk the clusters by `Segments` (`stages/segments.rs`), or take the run from the \
                     segment or cursor in hand; where a random-access read starts \
                     here, seeking once, add the file and door to ALLOWED_DOORS with the \
                     reason",
                );
            }
        }
    }

    /// A struct field declared `pub` on a struct that is not itself `pub`:
    /// wider than the crate that keeps its struct to itself. `pub(crate)` and
    /// `pub(super)` are the crate's, which frozen stage data may show its
    /// readers.
    fn fields(&mut self, file: &str, code: &Code) {
        for item in &code.structs {
            for field in &item.fields {
                let widened = field.vis == "pub" && item.vis != "pub";
                if !widened {
                    continue;
                }
                let allowed = ALLOWED_FIELDS
                    .iter()
                    .position(|&(f, s, _)| f == file && s == item.name);
                if let Some(at) = allowed {
                    self.used_fields.push(at);
                    continue;
                }
                self.fail(
                    file,
                    field.line,
                    &format!(
                        "field `{}` of `{}` is `{}`",
                        field.name, item.name, field.vis
                    ),
                    "make it `pub(crate)`, or `pub(super)` where only the stage's own \
                     submodules read it, or private behind a method where it is packed or \
                     guards an invariant; if a crate-private struct must show it past the \
                     crate, add it to ALLOWED_FIELDS with the reason",
                );
            }
        }
    }

    /// A function whose name has a word of [`BLOCKED_WORDS`] as one of its
    /// `_`-separated parts: `x_of`, `x_in`, `x_for`, `own_x`, `x_holding` or
    /// `x_at`. The bare name `at` is allowed.
    fn blocked_names(&mut self, file: &str, code: &Code) {
        for function in &code.functions {
            let name = function.name.as_str();
            if name == "at" || !name.split('_').any(|part| BLOCKED_WORDS.contains(&part)) {
                continue;
            }
            if let Some(at) = ALLOWED_NAMES
                .iter()
                .position(|&(f, n, _)| f == file && n == name)
            {
                self.used_names.push(at);
                continue;
            }
            self.fail(
                file,
                function.line,
                &format!("`fn {name}`"),
                "a per-owner view is `<owner>_<things>(owner)`, a containment lookup is \
                 `containing(position)` or `at(position)` on its own collection, \
                 `<thing>_containing(position)` derived from one, or the bare noun on \
                 another type, a predicate is `is_*`, a selector or a range is an argument \
                 to the plain noun, and a constructor is `new`, `from_*` or `with_*`; \
                 if Rust's own naming backs this one, add it to ALLOWED_NAMES with the reason",
            );
        }
    }

    /// `Range<usize>` in code outside the data module (`src/data/`) and
    /// `unit.rs`, where ids and lengths become numbers: a range into a
    /// table, which should be a range of its typed ids.
    fn usize_ranges(&mut self, file: &str, code: &Code) {
        if file.starts_with("src/data/") || file == "src/unit.rs" {
            return;
        }
        for (index, line) in code.lines.iter().enumerate() {
            let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if !squeezed.contains("Range<usize>") {
                continue;
            }
            let allowed = ALLOWED_USIZE_RANGES
                .iter()
                .position(|&(f, piece, _)| f == file && line.contains(piece));
            if let Some(at) = allowed {
                self.used_ranges.push(at);
                continue;
            }
            self.fail(
                file,
                index + 1,
                "`Range<usize>`",
                "index the table with its typed id (`define_id!` in `data`) and take a \
                 `Range<XId>`; a byte range in the text is a `Range<TextOffset>`; if it is \
                 neither, add it to ALLOWED_USIZE_RANGES with the reason",
            );
        }
    }

    /// An inherent function with no receiver that returns `Self`,
    /// `Option<Self>` or `Result<Self, …>`, not named `new`, `from_*` or
    /// `with_*`: a constructor with a name of its own.
    fn constructors(&mut self, file: &str, code: &Code) {
        for function in &code.functions {
            if function.in_trait || function.has_receiver || !function.returns_self {
                continue;
            }
            let name = function.name.as_str();
            // A name with a blocked word is reported as one already.
            let named = name == "new" || name.starts_with("from_") || name.starts_with("with_");
            if named || name.split('_').any(|part| BLOCKED_WORDS.contains(&part)) {
                continue;
            }
            let allowed = ALLOWED_CONSTRUCTORS
                .iter()
                .position(|&(f, n, _)| f == file && n == name);
            if let Some(at) = allowed {
                self.used_constructors.push(at);
                continue;
            }
            self.fail(
                file,
                function.line,
                &format!("constructor `fn {name}`"),
                "a constructor is `new`, or `from_*` where there are several, and `with_*` \
                 for an adjusted copy; a real conversion is a `From` impl; if Rust's own \
                 naming backs this one, add it to ALLOWED_CONSTRUCTORS with the reason",
            );
        }
    }

    /// An allowlist entry that excuses nothing: the rule is kept there now,
    /// so the entry goes.
    fn check_stale_entries(&mut self) {
        let lists: [(&str, &[Entry], &Vec<usize>); 6] = [
            ("ALLOWED_FIELDS", ALLOWED_FIELDS, &self.used_fields),
            ("ALLOWED_NAMES", ALLOWED_NAMES, &self.used_names),
            (
                "ALLOWED_USIZE_RANGES",
                ALLOWED_USIZE_RANGES,
                &self.used_ranges,
            ),
            (
                "ALLOWED_CONSTRUCTORS",
                ALLOWED_CONSTRUCTORS,
                &self.used_constructors,
            ),
            ("ALLOWED_DOORS", ALLOWED_DOORS, &self.used_doors),
            ("ALLOWED_STYLES", ALLOWED_STYLES, &self.used_styles),
        ];
        let mut stale = String::new();
        for (list, entries, used) in lists {
            for (at, (file, name, _)) in entries.iter().enumerate() {
                if !used.contains(&at) {
                    let _ = writeln!(
                        stale,
                        "{list}: `{file}` `{name}` excuses nothing\n    fix: remove the entry"
                    );
                }
            }
        }
        self.report.push_str(&stale);
    }

    fn fail(&mut self, file: &str, line: usize, what: &str, fix: &str) {
        let _ = writeln!(self.report, "{file}:{line}: {what}\n    fix: {fix}");
    }
}

// --------------------------------------------------------------------------
// Reading the code
// --------------------------------------------------------------------------

/// A source file's code, with its comments, literals and `#[cfg(test)]`
/// modules blanked, and the structs and functions the checks look at.
struct Code {
    lines: Vec<String>,
    structs: Vec<Struct>,
    functions: Vec<Function>,
}

struct Struct {
    name: String,
    vis: String,
    fields: Vec<Field>,
}

struct Field {
    name: String,
    vis: String,
    line: usize,
}

struct Function {
    name: String,
    line: usize,
    in_trait: bool,
    has_receiver: bool,
    returns_self: bool,
}

impl Code {
    fn new(text: &str) -> Self {
        let chars = blank_test_modules(blank(text));
        let lines = chars
            .iter()
            .collect::<String>()
            .lines()
            .map(str::to_owned)
            .collect();
        let mut code = Self {
            lines,
            structs: Vec::new(),
            functions: Vec::new(),
        };
        code.read_items(&chars);
        code
    }

    /// Finds the structs and functions of blanked code, and which blocks
    /// each function is in.
    fn read_items(&mut self, chars: &[char]) {
        let mut line = 1;
        // The blocks open, each with the depth it opened at and whether it
        // is a trait or a trait impl.
        let mut blocks: Vec<(usize, bool)> = Vec::new();
        let mut next_block: Option<bool> = None;
        let mut depth = 0;
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\n' => line += 1,
                '{' => {
                    depth += 1;
                    if let Some(is_trait) = next_block.take() {
                        blocks.push((depth, is_trait));
                    }
                }
                '}' => {
                    if blocks.last().is_some_and(|&(d, _)| d == depth) {
                        blocks.pop();
                    }
                    depth -= 1;
                }
                ';' => next_block = None,
                _ if keyword(chars, i, "impl") => {
                    let header = header(chars, i);
                    // A trait impl names its trait `for` a type; an `impl`
                    // in a signature is a type, and opens nothing.
                    if item_start(chars, i) {
                        next_block = Some(header.split_whitespace().any(|w| w == "for"));
                    }
                }
                _ if keyword(chars, i, "trait") => next_block = Some(true),
                _ if keyword(chars, i, "struct") => self.read_struct(chars, i, line),
                _ if keyword(chars, i, "fn") => {
                    let in_trait = blocks.last().is_some_and(|&(_, t)| t);
                    self.read_function(chars, i, line, in_trait);
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn read_struct(&mut self, chars: &[char], at: usize, line: usize) {
        let head = header(chars, at);
        let name = identifier(head["struct".len()..].trim_start());
        let vis = visibility_before(chars, at);
        let mut fields = Vec::new();
        let body = at + head.chars().count();
        if chars.get(body) == Some(&'{') {
            // Named fields, each at the body's own depth after an opening
            // brace or a comma.
            let mut line = line + head.matches('\n').count();
            let mut depth = 0;
            let mut starts = true;
            let mut i = body + 1;
            while i < chars.len() {
                let ch = chars[i];
                match ch {
                    '\n' => line += 1,
                    '(' | '[' | '<' | '{' => depth += 1,
                    '}' if depth == 0 => break,
                    '>' if chars[i - 1] == '-' => {}
                    ')' | ']' | '>' | '}' => depth -= 1,
                    ',' if depth == 0 => starts = true,
                    '#' if depth == 0 => {
                        // An attribute: past its brackets.
                        let mut brackets = 0;
                        while i < chars.len() {
                            match chars[i] {
                                '[' => brackets += 1,
                                ']' => {
                                    brackets -= 1;
                                    if brackets == 0 {
                                        break;
                                    }
                                }
                                '\n' => line += 1,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                    _ if starts && depth == 0 && !ch.is_whitespace() => {
                        starts = false;
                        let rest: String = chars[i..chars.len().min(i + 120)].iter().collect();
                        let vis = leading_visibility(&rest);
                        let after = rest[vis.len()..].trim_start();
                        let after = after.strip_prefix("r#").unwrap_or(after);
                        let name = identifier(after);
                        let tail = after[name.len()..].trim_start();
                        if !name.is_empty() && tail.starts_with(':') && !tail.starts_with("::") {
                            fields.push(Field { name, vis, line });
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        } else if let Some(open) = head.find('(') {
            // A tuple struct: its fields' visibilities, in order.
            for (n, part) in top_level_parts(&head[open + 1..]).iter().enumerate() {
                let vis = leading_visibility(part.trim_start());
                if !vis.is_empty() {
                    fields.push(Field {
                        name: n.to_string(),
                        vis,
                        line,
                    });
                }
            }
        }
        if !name.is_empty() {
            self.structs.push(Struct { name, vis, fields });
        }
    }

    fn read_function(&mut self, chars: &[char], at: usize, line: usize, in_trait: bool) {
        let head = header(chars, at);
        let name = identifier(head["fn".len()..].trim_start());
        if name.is_empty() {
            return;
        }
        let parameters = head.find('(').map_or("", |open| &head[open + 1..]);
        // The return type: what follows the parameters' closing parenthesis.
        let mut depth = 0;
        let close = parameters.char_indices().find_map(|(at, ch)| {
            match ch {
                '(' => depth += 1,
                ')' if depth == 0 => return Some(at),
                ')' => depth -= 1,
                _ => {}
            }
            None
        });
        let after = close.map_or("", |close| &parameters[close + 1..]);
        let first = top_level_parts(parameters)
            .first()
            .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let receiver = first.trim_start_matches('&');
        let receiver = receiver
            .strip_prefix('\'')
            .map_or(receiver, |r| r.split_once(' ').map_or("", |(_, rest)| rest));
        let receiver = receiver.trim_start().trim_start_matches("mut ");
        let has_receiver = receiver == "self" || receiver.starts_with("self:");
        let returns: String = after
            .split(" where ")
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let returns = returns.strip_prefix("->").unwrap_or("");
        let returns_self =
            returns == "Self" || returns == "Option<Self>" || returns.starts_with("Result<Self,");
        self.functions.push(Function {
            name,
            line,
            in_trait,
            has_receiver,
            returns_self,
        });
    }
}

/// `text` with every comment, and the contents of every string and
/// character literal, replaced by spaces, newlines kept, so that line
/// numbers and brace depths are the code's own.
fn blank(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    scan(&chars, |_, from, to| {
        for ch in &mut out[from..to] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
    });
    out
}

/// What [`scan`] finds in the code.
#[derive(Clone, Copy, PartialEq)]
enum Span {
    /// A comment from `//` to the line's end: `//`, `///` or `//!`.
    LineComment,
    /// A comment from `/*` to its `*/`, nested ones inside.
    BlockComment,
    /// A string or character literal, or the contents of one.
    Literal,
}

/// Calls `found` with each comment and each literal's contents in `chars`,
/// in order, as what it is and where it starts and ends.
fn scan(chars: &[char], mut found: impl FnMut(Span, usize, usize)) {
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        match chars[i] {
            '/' if next == Some('/') => {
                let end = (i..chars.len())
                    .find(|&j| chars[j] == '\n')
                    .unwrap_or(chars.len());
                found(Span::LineComment, i, end);
                i = end;
            }
            '/' if next == Some('*') => {
                let mut depth = 0;
                let mut j = i;
                while j < chars.len() {
                    if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                        depth += 1;
                        j += 2;
                    } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                found(Span::BlockComment, i, j.min(chars.len()));
                i = j;
            }
            'r' if matches!(next, Some('"' | '#')) && !identifier_char_before(chars, i) => {
                // A raw string, r"…" or r#"…"#.
                let mut j = i + 1;
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                while j < chars.len()
                    && !(chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')))
                {
                    j += 1;
                }
                let end = (j + 1 + hashes).min(chars.len());
                found(Span::Literal, i, end);
                i = end;
            }
            '"' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '"' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                found(Span::Literal, i + 1, j.min(chars.len()));
                i = j + 1;
            }
            '\'' => {
                // A character literal, not a lifetime: 'x', '\n', '\u{…}'.
                let close = if next == Some('\\') {
                    (i + 2..chars.len().min(i + 12)).find(|&j| chars[j] == '\'')
                } else {
                    (chars.get(i + 2) == Some(&'\'')).then_some(i + 2)
                };
                match close {
                    Some(close) => {
                        found(Span::Literal, i + 1, close);
                        i = close + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
}

/// `chars` with every module under `#[cfg(test)]` blanked, newlines kept.
fn blank_test_modules(mut chars: Vec<char>) -> Vec<char> {
    let attribute: Vec<char> = "#[cfg(test)]".chars().collect();
    let mut i = 0;
    while i + attribute.len() <= chars.len() {
        if chars[i..i + attribute.len()] != attribute[..] {
            i += 1;
            continue;
        }
        let rest: String = chars[i + attribute.len()..chars.len().min(i + attribute.len() + 40)]
            .iter()
            .collect();
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("pub(crate) ")
            .or_else(|| rest.strip_prefix("pub(super) "))
            .unwrap_or(rest);
        if !rest.starts_with("mod ") {
            i += 1;
            continue;
        }
        // Through the module's closing brace, or its semicolon for one in a
        // file of its own (which the file walk leaves out as `tests.rs`).
        let mut depth = 0;
        let mut j = i;
        while j < chars.len() {
            match chars[j] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                ';' if depth == 0 => break,
                _ => {}
            }
            j += 1;
        }
        let end = (j + 1).min(chars.len());
        for ch in &mut chars[i..end] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
        i = j + 1;
    }
    chars
}

/// Whether `line` constructs `record`: calls its `new`, or opens a literal
/// of it, `record {`, where `record` is a whole name, not a path's last
/// part, and not a type before a body (`-> &'a record {`, `impl record {`).
fn constructs(line: &str, record: &str) -> bool {
    line.match_indices(record).any(|(at, _)| {
        let before = &line[..at];
        let after = line[at + record.len()..].trim_start();
        let whole = !before.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':')
            && !after.starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if !whole {
            return false;
        }
        if after.starts_with("::new(") {
            return true;
        }
        if !after.starts_with('{') {
            return false;
        }
        let before = before.trim_end();
        let lifetime = before
            .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\''))
            .next()
            .is_some_and(|word| word.starts_with('\''));
        let keyword = ["struct", "enum", "impl", "for", "trait"]
            .iter()
            .any(|word| before == *word || before.ends_with(&format!(" {word}")));
        !(before.ends_with('&') || before.ends_with("->") || lifetime || keyword)
    })
}

/// Whether `line` defines a function whose name and opening parenthesis end
/// as `door` does, its receiver's dot aside: the door itself, not a call.
fn defines(line: &str, door: &str) -> bool {
    let door = door.trim_start_matches('.');
    line.match_indices("fn ").any(|(at, _)| {
        let rest = &line[at + 3..];
        let name = identifier(rest);
        rest[name.len()..].starts_with('(') && format!("{name}(").ends_with(door)
    })
}

/// Whether `word` is at `i` as a whole word.
fn keyword(chars: &[char], i: usize, word: &str) -> bool {
    let len = word.chars().count();
    chars
        .get(i..i + len)
        .is_some_and(|slice| slice.iter().copied().eq(word.chars()))
        && !identifier_char_before(chars, i)
        && !chars
            .get(i + len)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

/// Whether what is at `i` starts an item: the start of a line, after any
/// visibility, `unsafe` or attributes, rather than a type in a signature.
fn item_start(chars: &[char], i: usize) -> bool {
    let line_start = chars[..i]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |n| n + 1);
    let before: String = chars[line_start..i].iter().collect();
    let before = before.trim();
    before.is_empty() || before == "unsafe" || before.ends_with(']')
}

fn identifier_char_before(chars: &[char], i: usize) -> bool {
    i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')
}

/// The text from `at` up to, not including, the first `{` or `;` outside
/// parentheses and brackets.
fn header(chars: &[char], at: usize) -> String {
    let mut depth = 0;
    let mut end = at;
    while end < chars.len() {
        match chars[end] {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' | ';' if depth <= 0 => break,
            _ => {}
        }
        end += 1;
    }
    chars[at..end].iter().collect()
}

fn identifier(text: &str) -> String {
    text.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// The visibility written just before the item keyword at `at`.
fn visibility_before(chars: &[char], at: usize) -> String {
    let before: String = chars[at.saturating_sub(64)..at].iter().collect();
    let before = before.trim_end();
    if before.ends_with(')')
        && let Some(open) = before.rfind("pub(")
    {
        return before[open..].to_owned();
    }
    if before.ends_with("pub")
        && !before[..before.len() - 3].ends_with(|c: char| c.is_alphanumeric())
    {
        return "pub".to_owned();
    }
    String::new()
}

/// The visibility `text` starts with, if any.
fn leading_visibility(text: &str) -> String {
    if text.starts_with("pub(") {
        return text[..text.find(')').map_or(text.len(), |close| close + 1)].to_owned();
    }
    if text.starts_with("pub ") {
        return "pub".to_owned();
    }
    String::new()
}

/// `text` split at the commas outside any brackets, up to its first
/// unmatched closing parenthesis.
fn top_level_parts(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0;
    let mut previous = ' ';
    for ch in text.chars() {
        let arrow = previous == '-';
        previous = ch;
        match ch {
            ')' if depth == 0 => break,
            '>' if arrow => {}
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '>' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(part) = parts.last_mut() {
            part.push(ch);
        }
    }
    parts.retain(|part| !part.trim().is_empty());
    parts
}
