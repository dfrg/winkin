//! Normalization tests against ICU:
//! - the pairwise steps, built into NFD and NFC as UAX #15 defines them,
//!   normalize as ICU does: every character, every composing pair, a random
//!   corpus, and `NormalizationTest.txt` where the cargo registry has it;
//! - HarfBuzz's own properties match harfrust 0.14.0's.

use alloc::vec::Vec;
use core::array;
use std::path::{Path, PathBuf};
use std::{env, eprintln, fs};

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use icu_properties::props::{CanonicalCombiningClass, DefaultIgnorableCodePoint, GeneralCategory};
use icu_properties::{CodePointMapData, CodePointSetData};

use crate::unicode::normalize::*;

/// ICU's normalizers, the oracle.
const ICU_NFD: DecomposingNormalizerBorrowed<'static> = DecomposingNormalizerBorrowed::new_nfd();
const ICU_NFC: ComposingNormalizerBorrowed<'static> = ComposingNormalizerBorrowed::new_nfc();

/// Every character.
fn chars() -> impl Iterator<Item = char> {
    (0..=0x10FFFF_u32).filter_map(char::from_u32)
}

/// Appends `ch`, fully decomposed one step at a time through both halves,
/// to `out`.
fn push_decomposed(ch: char, out: &mut Vec<char>) {
    match decompose(ch) {
        None => out.push(ch),
        Some((a, b)) => {
            push_decomposed(a, out);
            if let Some(b) = b {
                push_decomposed(b, out);
            }
        }
    }
}

/// Returns the canonical decomposition of `text` (UAX #15 D68): each
/// character fully decomposed, then each run of non-starters stably sorted
/// by combining class.
fn nfd(text: &[char]) -> Vec<char> {
    let mut out = Vec::new();
    for &ch in text {
        push_decomposed(ch, &mut out);
    }
    for next in 1..out.len() {
        let class = combining_class(out[next]);
        if class == 0 {
            continue;
        }
        let mut to = next;
        while to > 0 && combining_class(out[to - 1]) > class {
            to -= 1;
        }
        out[to..=next].rotate_right(1);
    }
    out
}

/// Returns the canonical composition of `text` (UAX #15 D117).
///
/// Each character of the decomposition composes onto the last starter before
/// it where the pair has a primary composite, unless blocked: a character
/// between has class zero or a class at least its own.
fn nfc(text: &[char]) -> Vec<char> {
    let mut chars = nfd(text);
    let Some(&first) = chars.first() else {
        return chars;
    };
    let mut starter = 0;
    // The class of the last character kept since the starter. Text that
    // starts without a starter begins above every class, so nothing composes.
    let mut last = match combining_class(first) {
        0 => 0,
        _ => 256,
    };
    let mut kept = 1;
    for at in 1..chars.len() {
        let ch = chars[at];
        let class = u16::from(combining_class(ch));
        if (last == 0 || last < class)
            && let Some(composed) = compose(chars[starter], ch)
        {
            chars[starter] = composed;
            continue;
        }
        if class == 0 {
            starter = kept;
        }
        last = class;
        chars[kept] = ch;
        kept += 1;
    }
    chars.truncate(kept);
    chars
}

fn icu_nfd(text: &[char]) -> Vec<char> {
    ICU_NFD.normalize_iter(text.iter().copied()).collect()
}

fn icu_nfc(text: &[char]) -> Vec<char> {
    ICU_NFC.normalize_iter(text.iter().copied()).collect()
}

/// Every character alone decomposes and composes as ICU does, within
/// [`MAX_DECOMPOSITION`].
#[test]
fn every_character_normalizes_as_icu_does() {
    let mut longest = 0;
    let mut steps = 0;
    for ch in chars() {
        let text = [ch];
        let decomposed = nfd(&text);
        assert_eq!(decomposed, icu_nfd(&text), "NFD of U+{:04X}", u32::from(ch));
        assert_eq!(nfc(&text), icu_nfc(&text), "NFC of U+{:04X}", u32::from(ch));
        longest = longest.max(decomposed.len());
        let mut chain = 0;
        let mut at = ch;
        while let Some((a, _)) = decompose(at) {
            chain += 1;
            at = a;
        }
        steps = steps.max(chain);
    }
    assert_eq!(longest, MAX_DECOMPOSITION);
    assert!(steps <= MAX_DECOMPOSITION, "{steps} steps");
}

/// Composing a decomposed pair gives back the composite exactly where the
/// composite is its own NFC, which covers every composing pair, Hangul
/// included. Singletons compose from nothing.
#[test]
fn every_pair_that_composes_composes_back() {
    let (mut pairs, mut excluded, mut singletons) = (0, 0, 0);
    for ch in chars() {
        let text = [ch];
        match decompose(ch) {
            None => {}
            Some((a, None)) => {
                singletons += 1;
                assert_ne!(icu_nfc(&text), [ch], "U+{:04X}", u32::from(ch));
                assert_ne!(a, ch);
            }
            Some((a, Some(b))) => {
                let composes = icu_nfc(&text) == [ch];
                assert_eq!(
                    compose(a, b),
                    composes.then_some(ch),
                    "U+{:04X} from U+{:04X} U+{:04X}",
                    u32::from(ch),
                    u32::from(a),
                    u32::from(b)
                );
                if composes {
                    pairs += 1;
                } else {
                    excluded += 1;
                }
            }
        }
    }
    // Unicode 17: 11,172 Hangul syllables and 961 other primary
    // composites; the composition exclusions and the non-starter
    // decompositions that are pairs; and the singletons.
    assert_eq!((pairs, excluded, singletons), (11_172 + 961, 85, 1_035));
}

/// Every character's combining class is ICU's, except the 34 marks with a
/// class that Unicode 18 assigns, which ICU 2.3 (Unicode 17) leaves
/// unassigned.
#[test]
fn every_combining_class_is_unicodes() {
    let classes = CodePointMapData::<CanonicalCombiningClass>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();
    let mut newer = 0;
    for ch in chars() {
        #[allow(deprecated)]
        let expected = classes.get(ch).to_icu4c_value();
        let class = combining_class(ch);
        if class != expected && expected == 0 && categories.get(ch) == GeneralCategory::Unassigned {
            newer += 1;
            continue;
        }
        assert_eq!(class, expected, "U+{:04X}", u32::from(ch));
    }
    assert_eq!(newer, 34);
}

/// A pseudo-random source, the same every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or(0) % below.max(1)
    }
}

/// The characters the corpus draws from: those normalization acts on.
const POOL: &[char] = &[
    // Starters, and precomposed letters that decompose in one step, two
    // and three, with and without a composite after further marks.
    'a',
    'e',
    'i',
    'o',
    'u',
    'A',
    'E',
    'O',
    'c',
    'n',
    's',
    'z',
    '\u{E9}',
    '\u{E8}',
    '\u{EA}',
    '\u{FC}',
    '\u{1D8}',
    '\u{1E09}',
    '\u{1EB9}',
    '\u{1EC7}',
    '\u{1EBF}',
    '\u{C5}',
    '\u{1FB7}',
    '\u{3B1}',
    '\u{3B9}',
    '\u{3C9}',
    '\u{1F00}',
    '\u{1F02}',
    '\u{1F82}',
    '\u{439}',
    '\u{438}',
    // Arabic, Hebrew, Devanagari, Bengali, Kannada, Tibetan and Myanmar
    // letters, vowel signs and length marks, of which several compose.
    '\u{627}',
    '\u{648}',
    '\u{64A}',
    '\u{622}',
    '\u{5D0}',
    '\u{5E9}',
    '\u{915}',
    '\u{928}',
    '\u{929}',
    '\u{93E}',
    '\u{9C7}',
    '\u{9BE}',
    '\u{9D7}',
    '\u{9CB}',
    '\u{CC6}',
    '\u{CC2}',
    '\u{CD5}',
    '\u{CCA}',
    '\u{F40}',
    '\u{F71}',
    '\u{F72}',
    '\u{F74}',
    '\u{F80}',
    '\u{1025}',
    '\u{102E}',
    '\u{1026}',
    '\u{3042}',
    '\u{304B}',
    '\u{304C}',
    '\u{3099}',
    '\u{309A}',
    '\u{4E00}',
    // Marks of assorted classes: 1, 7, 9, 220, 230, 232, 233, 234, 240, 0
    // (a spacing mark and an enclosing one), Hebrew's and Arabic's fixed
    // positions, Thai's and Tibetan's.
    '\u{334}',
    '\u{93C}',
    '\u{94D}',
    '\u{323}',
    '\u{324}',
    '\u{327}',
    '\u{301}',
    '\u{300}',
    '\u{302}',
    '\u{303}',
    '\u{308}',
    '\u{30A}',
    '\u{313}',
    '\u{314}',
    '\u{342}',
    '\u{315}',
    '\u{35C}',
    '\u{35D}',
    '\u{345}',
    '\u{903}',
    '\u{20DD}',
    '\u{5B0}',
    '\u{5B4}',
    '\u{5B8}',
    '\u{5BC}',
    '\u{5C1}',
    '\u{5C2}',
    '\u{64B}',
    '\u{64E}',
    '\u{650}',
    '\u{651}',
    '\u{652}',
    '\u{653}',
    '\u{654}',
    '\u{655}',
    '\u{E38}',
    '\u{E3A}',
    '\u{E48}',
    '\u{F39}',
    '\u{1A60}',
    '\u{FC6}',
    // Hangul: leading, vowel and trailing jamo, old ones that compose to
    // nothing, and syllables with and without a trailing consonant.
    '\u{1100}',
    '\u{1112}',
    '\u{1113}',
    '\u{1161}',
    '\u{1175}',
    '\u{1176}',
    '\u{11A8}',
    '\u{11C2}',
    '\u{11C3}',
    '\u{AC00}',
    '\u{AC01}',
    '\u{D788}',
    '\u{D7A3}',
    // Composition exclusions, non-starter decompositions and singletons.
    '\u{958}',
    '\u{9DC}',
    '\u{F73}',
    '\u{F75}',
    '\u{F81}',
    '\u{344}',
    '\u{340}',
    '\u{341}',
    '\u{343}',
    '\u{374}',
    '\u{37E}',
    '\u{387}',
    '\u{1F71}',
    '\u{2000}',
    '\u{2001}',
    '\u{2126}',
    '\u{212A}',
    '\u{212B}',
    '\u{2ADC}',
    '\u{FB1D}',
    '\u{FB2A}',
    '\u{F900}',
    '\u{1D15E}',
    '\u{1D160}',
    '\u{1D165}',
    '\u{1D16E}',
    '\u{2F800}',
    // Joiners and a variation selector, which normalization passes over.
    '\u{200D}',
    '\u{34F}',
    '\u{FE00}',
];

/// Two hundred thousand sequences of one to sixteen characters from
/// [`POOL`] normalize as ICU does.
#[test]
fn a_random_corpus_normalizes_as_icu_does() {
    let mut random = Lcg(0x5EED_C0DE_F0E5);
    let mut text = Vec::new();
    let (mut decomposed, mut composed) = (0, 0);
    for _ in 0..200_000 {
        text.clear();
        for _ in 0..=random.next(16) {
            text.push(POOL[random.next(POOL.len())]);
        }
        let expected = icu_nfd(&text);
        assert_eq!(nfd(&text), expected, "NFD of {text:X?}");
        let composed_form = icu_nfc(&text);
        assert_eq!(nfc(&text), composed_form, "NFC of {text:X?}");
        decomposed += usize::from(expected != text);
        composed += usize::from(composed_form != text);
    }
    // Most sequences are changed by both, so the corpus reaches the rules.
    assert!(decomposed > 150_000, "{decomposed}");
    assert!(composed > 150_000, "{composed}");
}

/// Returns the path of the `NormalizationTest.txt` that icu_normalizer
/// 2.3's tests read, if the cargo registry holds it.
fn normalization_test() -> Option<PathBuf> {
    let home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(|home| Path::new(&home).join(".cargo"))
    })?;
    for index in fs::read_dir(home.join("registry").join("src"))
        .ok()?
        .flatten()
    {
        let file = index
            .path()
            .join("icu_normalizer-2.3.0")
            .join("tests")
            .join("data")
            .join("NormalizationTest.txt");
        if file.is_file() {
            return Some(file);
        }
    }
    None
}

/// Unicode's NFD and NFC tests (`NormalizationTest.txt`, 17.0) pass, where
/// the registry has them. Of each line's five columns, c3 and c5 are the
/// decompositions and c2 and c4 the compositions.
#[test]
fn unicodes_normalization_tests_pass() {
    let Some(path) = normalization_test() else {
        eprintln!("NormalizationTest.txt is not in the cargo registry; skipped");
        return;
    };
    let file = fs::read_to_string(&path).expect("the test file reads");
    let mut lines = 0;
    for line in file.lines() {
        let data = line.split('#').next().unwrap_or("").trim();
        if data.is_empty() || data.starts_with('@') {
            continue;
        }
        let columns: Vec<Vec<char>> = data
            .split(';')
            .take(5)
            .map(|column| {
                column
                    .split_whitespace()
                    .map(|hex| {
                        char::from_u32(u32::from_str_radix(hex, 16).expect("hex"))
                            .expect("a scalar value")
                    })
                    .collect()
            })
            .collect();
        let [c1, c2, c3, c4, c5] = &columns[..] else {
            panic!("five columns: {line}");
        };
        for source in [c1, c2, c3] {
            assert_eq!(&nfd(source), c3, "NFD, {line}");
            assert_eq!(&nfc(source), c2, "NFC, {line}");
        }
        for source in [c4, c5] {
            assert_eq!(&nfd(source), c5, "NFD, {line}");
            assert_eq!(&nfc(source), c4, "NFC, {line}");
        }
        lines += 1;
    }
    assert!(lines > 19_000, "{lines} lines");
}

/// For every character, the modified combining class matches harfrust's
/// table (`MODIFIED_COMBINING_CLASS`, transcribed), including its three
/// single-mark entries.
#[test]
fn modified_classes_are_harfrusts() {
    let mut table: [u8; 256] = array::from_fn(|class| class as u8);
    let hebrew = [
        22, 15, 16, 17, 23, 18, 19, 20, 21, 14, 24, 12, 25, 13, 10, 11, 26,
    ];
    table[10..27].copy_from_slice(&hebrew);
    let arabic = [28, 29, 30, 31, 32, 33, 27, 34, 35];
    table[27..36].copy_from_slice(&arabic);
    table[84] = 0;
    table[91] = 0;
    table[103] = 3;
    table[130] = 132;
    table[132] = 131;
    for ch in chars() {
        let expected = match ch {
            '\u{1A60}' | '\u{0FC6}' => 254,
            '\u{0F39}' => 127,
            _ => table[usize::from(combining_class(ch))],
        };
        assert_eq!(
            modified_combining_class(ch),
            expected,
            "U+{:04X}",
            u32::from(ch)
        );
    }
}

/// HarfBuzz's default-ignorables are Unicode's less exactly the Hangul
/// fillers and the shorthand format controls.
#[test]
fn harfbuzzs_default_ignorables_are_unicodes_less_its_exceptions() {
    let unicode = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    let mut left_visible = Vec::new();
    for ch in chars() {
        let harfbuzz = is_default_ignorable(ch);
        assert!(
            !harfbuzz || unicode.contains(ch),
            "U+{:04X} is not Unicode's",
            u32::from(ch)
        );
        if unicode.contains(ch) && !harfbuzz {
            left_visible.push(u32::from(ch));
        }
    }
    assert_eq!(
        left_visible,
        [
            0x115F, 0x1160, 0x3164, 0xFFA0, 0x1BCA0, 0x1BCA1, 0x1BCA2, 0x1BCA3
        ]
    );
}

/// The variation selectors are the 256, and all are marks and
/// default-ignorables.
#[test]
fn variation_selectors_are_the_256() {
    let selectors: Vec<char> = chars().filter(|&ch| is_variation_selector(ch)).collect();
    assert_eq!(selectors.len(), 256);
    assert!(
        selectors
            .iter()
            .all(|&ch| is_mark(ch) && is_default_ignorable(ch))
    );
}
