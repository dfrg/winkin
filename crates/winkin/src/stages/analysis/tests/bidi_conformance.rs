//! The bidi resolver against Unicode's conformance files, `BidiTest.txt`
//! and `BidiCharacterTest.txt`, per character and per cluster.

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use icu_properties::props::BidiClass as IcuBidiClass;
use icu_properties::props::{BidiMirroringGlyph, BidiPairedBracketType, EnumeratedProperty};

use crate::stages::analysis::bidi::{BidiScratch, BidiUnit, reset_separators, resolve_bidi};
use crate::tests::unicode_test_data;
use crate::unicode::{self, BidiBracket, BidiClass, cluster_bidi_class, reorder_bidi};

/// Returns ICU's class of `ch`.
// ICU4C's numbering is the point: it is the resolver's, and the tables'.
// icu_properties deprecates the accessor pending a use for it (icu4x#6067).
#[allow(deprecated)]
fn icu_class(ch: char) -> BidiClass {
    BidiClass::new(IcuBidiClass::for_char(ch).to_icu4c_value())
}

/// Returns whether L1 resets a character of class `class` at the end of a
/// line to the paragraph's level: white space and the isolate formatting
/// characters, and a segment separator, which L1 resets wherever it falls,
/// so that a walk back from the end passes through a tab rather than
/// stopping at it. Line layout resets by Chrome's rule rather than by class,
/// so only these tests ask.
fn needs_trailing_neutral_reset(class: BidiClass) -> bool {
    const RESET: u32 = BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
        | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
        | BidiClass::FIRST_STRONG_ISOLATE.mask()
        | BidiClass::POP_DIRECTIONAL_ISOLATE.mask()
        | BidiClass::WHITE_SPACE.mask()
        | BidiClass::SEGMENT_SEPARATOR.mask();
    class.mask() & RESET != 0
}

/// Every class constant carries ICU4C's `UCharDirection` value, which is what
/// the generated tables store and what `icu_class` passes through unmapped.
#[test]
#[allow(deprecated)] // As in `icu_class`.
fn the_classes_are_numbered_as_icu4c() {
    use icu_properties::props::BidiClass as Icu;
    let pairs = [
        (BidiClass::LEFT_TO_RIGHT, Icu::LeftToRight),
        (BidiClass::RIGHT_TO_LEFT, Icu::RightToLeft),
        (BidiClass::EUROPEAN_NUMBER, Icu::EuropeanNumber),
        (BidiClass::EUROPEAN_SEPARATOR, Icu::EuropeanSeparator),
        (BidiClass::EUROPEAN_TERMINATOR, Icu::EuropeanTerminator),
        (BidiClass::ARABIC_NUMBER, Icu::ArabicNumber),
        (BidiClass::COMMON_SEPARATOR, Icu::CommonSeparator),
        (BidiClass::PARAGRAPH_SEPARATOR, Icu::ParagraphSeparator),
        (BidiClass::SEGMENT_SEPARATOR, Icu::SegmentSeparator),
        (BidiClass::WHITE_SPACE, Icu::WhiteSpace),
        (BidiClass::OTHER_NEUTRAL, Icu::OtherNeutral),
        (
            BidiClass::LEFT_TO_RIGHT_EMBEDDING,
            Icu::LeftToRightEmbedding,
        ),
        (BidiClass::LEFT_TO_RIGHT_OVERRIDE, Icu::LeftToRightOverride),
        (BidiClass::ARABIC_LETTER, Icu::ArabicLetter),
        (
            BidiClass::RIGHT_TO_LEFT_EMBEDDING,
            Icu::RightToLeftEmbedding,
        ),
        (BidiClass::RIGHT_TO_LEFT_OVERRIDE, Icu::RightToLeftOverride),
        (BidiClass::POP_DIRECTIONAL_FORMAT, Icu::PopDirectionalFormat),
        (BidiClass::NONSPACING_MARK, Icu::NonspacingMark),
        (BidiClass::BOUNDARY_NEUTRAL, Icu::BoundaryNeutral),
        (BidiClass::FIRST_STRONG_ISOLATE, Icu::FirstStrongIsolate),
        (BidiClass::LEFT_TO_RIGHT_ISOLATE, Icu::LeftToRightIsolate),
        (BidiClass::RIGHT_TO_LEFT_ISOLATE, Icu::RightToLeftIsolate),
        (
            BidiClass::POP_DIRECTIONAL_ISOLATE,
            Icu::PopDirectionalIsolate,
        ),
    ];
    for (i, (ours, icu)) in pairs.into_iter().enumerate() {
        assert_eq!(ours.get(), icu.to_icu4c_value(), "{icu:?}");
        // And in order, with no gaps: L = 0 through PDI = 22.
        assert_eq!(usize::from(ours.get()), i, "{icu:?}");
    }
}

/// Resolves `units` as one paragraph through L1's separator half, as
/// analysis does, and returns the base level.
pub(super) fn resolve_paragraph(
    scratch: &mut BidiScratch,
    units: &[BidiUnit],
    brackets: &[BidiBracket],
    base: Option<u8>,
    levels: &mut [u8],
) -> u8 {
    let mask = units.iter().fold(0, |mask, unit| mask | unit.class.mask());
    let base = resolve_bidi(scratch, units, brackets, mask, base, levels);
    reset_separators(units, levels, base, |_, _| {});
    base
}

/// The clusters' units of `classes`, one each.
fn as_units(classes: &[BidiClass]) -> Vec<BidiUnit> {
    let unit = |&class| BidiUnit {
        class,
        control: false,
    };
    classes.iter().map(unit).collect()
}

/// An isolate that ends a left-to-right paragraph after right-to-left
/// text has its controls at the paragraph's level.
///
/// `R LRI L PDI` resolves to 1, 0, 2, 0. The LRI and PDI are neutrals
/// between R and the paragraph's end, so N2 sets them at the embedding
/// level, as ICU resolves them for Blink.
#[test]
fn an_isolate_ending_a_paragraph_after_rtl_has_its_controls_at_the_paragraph_level() {
    let classes = [
        BidiClass::RIGHT_TO_LEFT,
        BidiClass::LEFT_TO_RIGHT_ISOLATE,
        BidiClass::LEFT_TO_RIGHT,
        BidiClass::POP_DIRECTIONAL_ISOLATE,
    ];
    let mut levels = [0; 4];
    let units = as_units(&classes);
    let base = resolve_paragraph(&mut BidiScratch::new(), &units, &[], Some(0), &mut levels);
    assert_eq!((base, levels), (0, [1, 0, 2, 0]));
}

/// The paired-bracket role of a character, without a position.
fn bidi_bracket_from_icu(ch: char) -> Option<(char, bool)> {
    let bracket = BidiMirroringGlyph::for_char(ch);
    // Both ends of a pair must name the same character, which N0 matches on.
    // For an opener that is the character it mirrors to; for a closer it is the
    // closer itself, not its mirror.
    match bracket.paired_bracket_type {
        BidiPairedBracketType::Open => bracket.mirroring_glyph.map(|closing| (closing, true)),
        BidiPairedBracketType::Close => Some((ch, false)),
        _ => None,
    }
}

fn parse_index_list(input: &str) -> Vec<u32> {
    input
        .split_whitespace()
        .map(|s| s.parse::<u32>().unwrap())
        .collect()
}

fn parse_level_list(input: &str) -> Vec<String> {
    input.split_whitespace().map(str::to_owned).collect()
}

fn resolve_trailing_neutrals(levels: &mut [u8], classes: &[BidiClass], base_level: u8) {
    for i in (0..classes.len()).rev() {
        let class = classes[i];
        if class.is_removed_by_x9() {
            continue;
        }
        if needs_trailing_neutral_reset(class) {
            levels[i] = base_level;
        } else {
            break;
        }
    }
}

/// The lines of the conformance file `name` that are neither blank nor a
/// comment, trimmed, or `None` where the file is absent.
fn test_data_lines(name: &str) -> Option<Vec<String>> {
    let text = unicode_test_data(name)?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn bidi_test() {
    let Some(lines) = test_data_lines("BidiTest.txt") else {
        return;
    };
    let mut state = TestState::new();
    let mut codepoints = Vec::new();
    let mut levels = Vec::new();
    let mut order = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("@Levels:\t") {
            levels = parse_level_list(rest);
            continue;
        }
        if let Some(rest) = line.strip_prefix("@Reorder:") {
            order = parse_index_list(rest);
            continue;
        }
        codepoints.clear();
        let (types, dirs_hex) = line.split_once("; ").unwrap();
        codepoints.extend(types.split_whitespace().map(char_from_type));
        let dirs = u8::from_str_radix(dirs_hex.trim(), 16).unwrap();
        state.run_dirs(&codepoints, &levels, &order, dirs);
    }
    state.finish();
}

#[test]
fn bidi_character_test() {
    let Some(lines) = test_data_lines("BidiCharacterTest.txt") else {
        return;
    };
    let mut state = TestState::new();
    for line in lines {
        let parts = line.split(';').collect::<Vec<_>>();
        // BidiCharacterTest fields:
        // [0] code points, [1] paragraph direction hint, [2] resolved base level,
        // [3] expected levels, [4] expected visual reorder.
        assert_eq!(parts.len(), 5, "invalid BidiCharacterTest line: {line}");
        let codepoints = parts[0]
            .split_whitespace()
            .map(|codepoint| {
                let cp = u32::from_str_radix(codepoint, 16).unwrap();
                char::from_u32(cp).unwrap()
            })
            .collect::<Vec<char>>();
        let dir = match parts[1].trim() {
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        };
        let base_level = parts[2].trim().parse::<u8>().unwrap();
        let levels = parse_level_list(parts[3]);
        let order = parse_index_list(parts[4]);
        state.run(Some(base_level), &codepoints, &levels, &order, dir);
    }
    state.finish();
}

/// Resolving one unit per grapheme cluster, its first character's class,
/// gives each cluster the level the per-character answer gives each of its
/// characters.
///
/// The crate hands the resolver one unit per cluster, which is cheaper and
/// keeps combining marks out of the algorithm, and conformance is proven per
/// character, so this runs every string of both conformance files both
/// ways: the clusters are the graphemes analysis makes, each
/// resolved as its first character with its bracket where that is one, and
/// each compared with the file's levels for its characters, the ones X9
/// removes aside. Line L1 is applied to both as the files apply it, to the
/// trailing whitespace of the whole string.
///
/// Where a cluster's characters are not all at one level in the file's
/// answer, no one level per cluster could give it; there are none such, and
/// every cluster agrees, once a space or a separator carrying a mark is taken
/// as the neutral the pair resolves as ([`cluster_class`], the rule analysis
/// resolves each cluster by).
#[test]
fn per_cluster_resolution_matches_the_per_character_answer() {
    let (Some(characters), Some(classes)) = (
        test_data_lines("BidiCharacterTest.txt"),
        test_data_lines("BidiTest.txt"),
    ) else {
        return;
    };
    let mut grouped = ByCluster::new();
    for line in characters {
        let parts = line.split(';').collect::<Vec<_>>();
        let text: String = parts[0]
            .split_whitespace()
            .map(|cp| char::from_u32(u32::from_str_radix(cp, 16).unwrap()).unwrap())
            .collect();
        let base = match parts[1].trim() {
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        };
        grouped.compare(&text, base, &parse_level_list(parts[3]));
    }
    // What this took in, so that the file changing or the segmenter
    // grouping otherwise is seen: every cluster compared but those whose
    // characters X9 removes, and none of them of mixed levels.
    assert_eq!(grouped.finish(), (717_472, 717_260, 0));

    let mut grouped = ByCluster::new();
    let mut levels = Vec::new();
    for line in classes {
        if let Some(rest) = line.strip_prefix("@Levels:\t") {
            levels = parse_level_list(rest);
            continue;
        }
        if line.starts_with('@') {
            continue;
        }
        let (types, dirs_hex) = line.split_once("; ").unwrap();
        let text: String = types.split_whitespace().map(char_from_type).collect();
        let dirs = u8::from_str_radix(dirs_hex.trim(), 16).unwrap();
        for (mask, base) in [(1, None), (2, Some(0)), (4, Some(1))] {
            if dirs & mask != 0 {
                grouped.compare(&text, base, &levels);
            }
        }
    }
    // The class strings stand for each class by one character, many of them
    // X9 removes, so fewer are compared.
    assert_eq!(grouped.finish(), (3_012_609, 2_189_640, 0));
}

/// Compares the per-cluster resolution of strings with the per-character
/// answer, and counts what it compared.
struct ByCluster {
    scratch: BidiScratch,
    levels: Vec<u8>,
    clusters: usize,
    compared: usize,
    mixed: usize,
    disagreements: Vec<(String, usize, u8, String)>,
}

impl ByCluster {
    fn new() -> Self {
        Self {
            scratch: BidiScratch::new(),
            levels: Vec::new(),
            clusters: 0,
            compared: 0,
            mixed: 0,
            disagreements: Vec::new(),
        }
    }

    /// Resolves `text` a cluster at a time under `base`, and compares each
    /// cluster with `expected`, the per-character answer.
    fn compare(&mut self, text: &str, base: Option<u8>, expected: &[String]) {
        // Where each cluster starts, in characters, and the end.
        let starts: Vec<usize> = unicode::graphemes(text)
            .map(|(byte, _)| byte)
            .chain([text.len()])
            .map(|byte| text[..byte].chars().count())
            .collect();
        let chars: Vec<char> = text.chars().collect();
        // Each cluster resolves as analysis resolves it, by the module's own
        // rule, from ICU's classes.
        let units: Vec<BidiClass> = starts
            .windows(2)
            .map(|pair| {
                let cluster = &chars[pair[0]..pair[1]];
                cluster_bidi_class(
                    icu_class(cluster[0]),
                    cluster[1..].iter().map(|&ch| icu_class(ch)),
                )
            })
            .collect();
        let brackets: Vec<BidiBracket> = starts
            .windows(2)
            .enumerate()
            .filter_map(|(at, pair)| {
                bidi_bracket_from_icu(chars[pair[0]]).map(|(closing, is_open)| BidiBracket {
                    index: at as u32,
                    closing,
                    is_open,
                })
            })
            .collect();
        self.levels.clear();
        self.levels.resize(units.len(), 0);
        let resolved = resolve_paragraph(
            &mut self.scratch,
            &as_units(&units),
            &brackets,
            base,
            &mut self.levels,
        );
        resolve_trailing_neutrals(&mut self.levels, &units, resolved);
        for (at, pair) in starts.windows(2).enumerate() {
            self.clusters += 1;
            let wanted: Vec<&str> = expected[pair[0]..pair[1]]
                .iter()
                .map(String::as_str)
                .filter(|level| *level != "x")
                .collect();
            let Some(first) = wanted.first() else {
                continue;
            };
            if wanted.iter().any(|level| level != first) {
                self.mixed += 1;
                continue;
            }
            self.compared += 1;
            if *first != self.levels[at].to_string() {
                self.disagreements
                    .push((text.to_owned(), at, self.levels[at], first.to_string()));
            }
        }
    }

    /// Clusters met, compared and of mixed levels, where every one compared
    /// agreed.
    fn finish(&self) -> (usize, usize, usize) {
        assert!(
            self.disagreements.is_empty(),
            "{} of {} clusters resolve otherwise than their characters: {:?}",
            self.disagreements.len(),
            self.compared,
            &self.disagreements[..self.disagreements.len().min(10)]
        );
        (self.clusters, self.compared, self.mixed)
    }
}

struct TestState {
    scratch: BidiScratch,
    levels: Vec<u8>,
    failures: Vec<Failure>,
    count: usize,
    failure_count: usize,
}

impl TestState {
    fn new() -> Self {
        Self {
            scratch: BidiScratch::new(),
            levels: Vec::new(),
            failures: Vec::new(),
            count: 0,
            failure_count: 0,
        }
    }

    fn run_dirs(&mut self, codepoints: &[char], levels: &[String], order: &[u32], dirs: u8) {
        for (mask, base_level) in [(1, None), (2, Some(0)), (4, Some(1))] {
            if dirs & mask != 0 {
                self.run(None, codepoints, levels, order, base_level);
            }
        }
    }

    fn run(
        &mut self,
        expected_base_level: Option<u8>,
        codepoints: &[char],
        levels: &[String],
        order: &[u32],
        input_base_level: Option<u8>,
    ) {
        let index = self.count;
        self.count += 1;
        let classes = codepoints
            .iter()
            .copied()
            .map(icu_class)
            .collect::<Vec<_>>();
        let brackets = codepoints
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(i, ch)| {
                bidi_bracket_from_icu(ch).map(|(closing, is_open)| BidiBracket {
                    index: i as u32,
                    closing,
                    is_open,
                })
            })
            .collect::<Vec<_>>();
        self.levels.clear();
        self.levels.resize(classes.len(), 0);
        let test_base_level = resolve_paragraph(
            &mut self.scratch,
            &as_units(&classes),
            &brackets,
            input_base_level,
            &mut self.levels,
        );
        let mut test_levels = self.levels.clone();
        resolve_trailing_neutrals(&mut test_levels, &classes, test_base_level);
        let test_levels_str = test_levels
            .iter()
            .enumerate()
            .map(|(i, level)| {
                if classes[i].is_removed_by_x9() {
                    "x".to_owned()
                } else {
                    level.to_string()
                }
            })
            .collect::<Vec<_>>();
        let mut test_order = vec![0; test_levels.len()];
        reorder_bidi(&mut test_order, |i| test_levels[i]);
        test_order.retain(|i| !classes[*i as usize].is_removed_by_x9());
        if test_levels_str != levels
            || test_order != order
            || expected_base_level.is_some_and(|expected| expected != test_base_level)
        {
            self.failure_count += 1;
            if self.failure_count <= 25 {
                self.failures.push(Failure {
                    index,
                    codepoints: codepoints.to_owned(),
                    exp_levels: levels.to_owned(),
                    levels: test_levels_str,
                    exp_order: order.to_owned(),
                    order: test_order,
                    exp_base_level: expected_base_level,
                    base_level: test_base_level,
                });
            }
        }
    }

    fn finish(&self) {
        if self.failure_count != 0 {
            panic!(
                "{}/{} passed, {} failed\n{:?}",
                self.count - self.failure_count,
                self.count,
                self.failure_count,
                self.failures
            );
        }
    }
}

#[derive(Debug)]
// Fields are only read by the Debug impl when printing
// failures and rustc ignores those uses.
#[allow(dead_code)]
struct Failure {
    index: usize,
    codepoints: Vec<char>,
    exp_levels: Vec<String>,
    levels: Vec<String>,
    exp_order: Vec<u32>,
    order: Vec<u32>,
    exp_base_level: Option<u8>,
    base_level: u8,
}

fn char_from_type(ty: &str) -> char {
    char::from_u32(match ty {
        "ON" => '|' as u32,
        "L" => 0x200E,
        "R" => 0x200F,
        "AN" => 0x661,
        "EN" => '0' as u32,
        "AL" => 0x61C,
        "NSM" => 0x300,
        "CS" => ',' as u32,
        "ES" => '+' as u32,
        "ET" => '$' as u32,
        "BN" => 3,
        "S" => '\t' as u32,
        "WS" => ' ' as u32,
        "B" => '\n' as u32,
        "RLO" => 0x202E,
        "RLE" => 0x202B,
        "LRO" => 0x202D,
        "LRE" => 0x202A,
        "PDF" => 0x202C,
        "FSI" => 0x2068,
        "LRI" => 0x2066,
        "PDI" => 0x2069,
        "RLI" => 0x2067,
        _ => 0,
    })
    .unwrap()
}
