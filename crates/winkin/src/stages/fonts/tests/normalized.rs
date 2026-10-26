//! Tests of coverage's replay of harfrust's normalizer:
//! - normalization cases over fonts built in memory that map exactly what a
//!   case names;
//! - clusters drawn from a mixture of forms;
//! - Hangul as harfrust's Hangul shaper takes it;
//! - agreement with harfrust: over random fonts and the clusters
//!   normalization turns on, a font covers a cluster exactly where harfrust
//!   shapes it with no `.notdef`.

use std::path::{Path, PathBuf};
use std::{env, eprintln, fs};

use harfrust::Script;
use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};

use super::*;
use crate::unicode;

/// Returns a font named `family` that maps exactly `chars`.
fn named(family: &str, chars: &[char]) -> TestFont {
    let mut code_points: Vec<u32> = chars.iter().map(|&ch| u32::from(ch)).collect();
    code_points.sort_unstable();
    code_points.dedup();
    let ranges: Vec<(u32, u32)> = code_points.iter().map(|&u| (u, u)).collect();
    TestFont::new(family, &ranges)
}

/// Returns a font that maps exactly `chars`.
fn mapping(chars: &[char]) -> TestFont {
    named("Test Normalized", chars)
}

/// Returns whether a font mapping exactly `chars` covers the cluster `text`.
///
/// Asserts that harfrust agrees.
fn covers(chars: &[char], text: &str) -> bool {
    let bytes = mapping(chars).build();
    let font = fontwich::Font::from_data(bytes.clone(), 0);
    let covered = coverage::covers(&font, text, &mut None);
    assert_eq!(
        covered,
        harfrust_draws(&bytes, text),
        "{text:?} in a font of {chars:?}"
    );
    covered
}

const E_ACUTE: char = '\u{E9}';
const ACUTE: char = '\u{301}';
const DIAERESIS: char = '\u{308}';

/// A font with the precomposed letter covers it. So does one with only its
/// pieces, which harfrust decomposes it into. One with neither does not.
#[test]
fn a_letter_is_covered_whole_or_in_pieces() {
    assert!(covers(&[E_ACUTE], "\u{E9}"));
    assert!(covers(&['e', ACUTE], "\u{E9}"));
    assert!(!covers(&['e'], "\u{E9}"));
    assert!(!covers(&['x'], "\u{E9}"));
}

/// Decomposed text is covered by a font with only the precomposed letter,
/// which harfrust composes it into.
#[test]
fn decomposed_text_is_covered_composed() {
    assert!(covers(&[E_ACUTE], "e\u{301}"));
    assert!(!covers(&['e'], "e\u{301}"));
}

/// A singleton decomposition is followed: U+212B is U+00C5, so a font with
/// U+00C5, or with its pieces, draws it.
#[test]
fn a_singleton_is_followed() {
    assert!(covers(&['\u{C5}'], "\u{212B}"));
    assert!(covers(&['A', '\u{30A}'], "\u{212B}"));
}

/// `ǘ` in a font with `ü` and U+0301 and neither `ǘ` nor U+0308 is covered.
/// harfrust decomposes it one step, to `ü` and U+0301, and stops there
/// because the font has `ü`. Neither the fully decomposed nor the composed
/// form is in the font. Written decomposed or half composed, it is covered
/// alike, since harfrust composes what the font has.
#[test]
fn a_cluster_in_a_mixture_of_forms_is_covered() {
    let font = ['\u{FC}', ACUTE];
    assert!(covers(&font, "\u{1D8}"));
    assert!(covers(&font, "u\u{308}\u{301}"));
    assert!(covers(&font, "\u{FC}\u{301}"));
    // U+1E09, c with cedilla and acute, in a font with `ç` and the acute.
    assert!(covers(&['\u{E7}', ACUTE], "\u{1E09}"));
    assert!(covers(&['\u{E7}', ACUTE], "c\u{327}\u{301}"));
    assert!(!covers(&['u', DIAERESIS], "\u{1D8}"));
}

/// A cluster is composed a pair at a time, and each composite must be one
/// the font maps. `ǘ` comes from `ü` and U+0301, and from `u` and its two
/// marks only where the font has the `ü` the first pair makes. Its composed
/// form alone is not enough.
#[test]
fn a_cluster_is_composed_a_pair_at_a_time() {
    assert!(covers(&['\u{1D8}'], "\u{FC}\u{301}"));
    assert!(!covers(&['\u{1D8}'], "u\u{308}\u{301}"));
    assert!(covers(&['\u{FC}', '\u{1D8}'], "u\u{308}\u{301}"));
    // `ệ` comes from `ẹ` and U+0302, and from `e` and its marks in either
    // order only where the font has the `ẹ` between.
    assert!(covers(&['\u{1EC7}'], "\u{1EB9}\u{302}"));
    assert!(!covers(&['\u{1EC7}'], "e\u{323}\u{302}"));
    let font = ['\u{1EB9}', '\u{1EC7}'];
    assert!(covers(&font, "e\u{323}\u{302}"));
    assert!(covers(&font, "e\u{302}\u{323}"));
}

/// A mark composes onto its starter past a mark of a lower class, but not
/// past one of the same class or a starter:
/// - U+0323 (220) sorts before U+0302 (230), so `ê` composes from `e` past
///   it;
/// - a second mark of U+0301's class blocks it;
/// - the combining grapheme joiner, of class zero, is a starter of its own
///   and blocks it too.
#[test]
fn a_mark_composes_where_nothing_blocks_it() {
    assert!(covers(&['\u{EA}', '\u{323}'], "e\u{323}\u{302}"));
    assert!(covers(&['\u{EA}', '\u{323}'], "e\u{302}\u{323}"));
    assert!(!covers(&['\u{E9}', '\u{300}'], "e\u{300}\u{301}"));
    assert!(!covers(&['\u{E9}', '\u{34F}'], "e\u{34F}\u{301}"));
    assert!(covers(&['e', '\u{34F}', ACUTE], "e\u{34F}\u{301}"));
}

/// A space the font lacks is drawn with U+0020's glyph, and U+2011 with
/// U+2010's. U+2000, which decomposes to U+2002, is drawn with U+2002's
/// where the font has that and not U+0020.
#[test]
fn a_space_and_a_hyphen_are_faked() {
    assert!(covers(&[' '], "\u{2003}"));
    assert!(covers(&[' '], "\u{A0}"));
    assert!(!covers(&['a'], "\u{2003}"));
    assert!(covers(&['\u{2010}'], "\u{2011}"));
    assert!(!covers(&['-'], "\u{2011}"));
    assert!(covers(&['\u{2002}'], "\u{2000}"));
    assert!(!covers(&['\u{2003}'], "\u{2002}"));
}

/// A default-ignorable that harfrust hides does not keep a font from a
/// cluster. One it draws, a Hangul filler, does. The fourth Mongolian free
/// variation selector is hidden, as harfrust 0.15.0 hides it.
#[test]
fn a_default_ignorable_harfrust_hides_keeps_no_font_from_a_cluster() {
    assert!(covers(&['a', 'b'], "a\u{200D}b"));
    assert!(covers(&['\u{1820}'], "\u{1820}\u{180F}"));
    assert!(covers(&['a'], "a\u{200D}"));
    assert!(covers(&['a', ACUTE], "a\u{34F}\u{301}"));
    assert!(!covers(&['\u{1100}'], "\u{1100}\u{1160}"));
    assert!(covers(&['\u{1100}', '\u{1160}'], "\u{1100}\u{1160}"));
}

/// A character none of whose forms the font has is not covered.
#[test]
fn what_nothing_draws_is_not_covered() {
    assert!(!covers(&['a'], "a\u{4E2D}"));
    assert!(!covers(&['a'], "\u{4E2D}"));
    assert!(!covers(&['a', ACUTE], "b\u{301}"));
}

/// A variation sequence is taken as written, as harfrust takes a cluster
/// with a selector: its base where the font maps it, the selector hidden,
/// and nothing decomposed.
#[test]
fn a_variation_sequence_is_taken_as_written() {
    assert!(covers(&['\u{2603}'], "\u{2603}\u{FE0F}"));
    assert!(covers(&['\u{2603}', '\u{FE0F}'], "\u{2603}\u{FE0F}"));
    assert!(covers(&[E_ACUTE], "\u{E9}\u{FE00}"));
    assert!(!covers(&['e', ACUTE], "\u{E9}\u{FE00}"));
    // Composition still runs over it.
    assert!(covers(&[E_ACUTE], "e\u{301}\u{FE00}"));
}

/// Hangul is covered as harfrust's Hangul shaper takes it:
/// - jamo composed into a syllable the font has;
/// - a syllable the font lacks drawn as the jamo it has;
/// - a syllable and a trailing consonant composed;
/// - a syllable with a trailing consonant that the font lacks drawn as the
///   syllable without it and the consonant.
#[test]
fn hangul_is_taken_as_its_shaper_takes_it() {
    let (ga, gak) = ('\u{AC00}', '\u{AC01}');
    assert!(covers(&[ga], "\u{1100}\u{1161}"));
    assert!(covers(&[gak], "\u{1100}\u{1161}\u{11A8}"));
    assert!(!covers(&[ga], "\u{1100}\u{1161}\u{11A8}"));
    assert!(covers(&['\u{1100}', '\u{1161}'], "\u{AC00}"));
    assert!(covers(&['\u{1100}', '\u{1161}', '\u{11A8}'], "\u{AC01}"));
    assert!(covers(&[gak], "\u{AC00}\u{11A8}"));
    assert!(covers(&[ga, '\u{11A8}'], "\u{AC01}"));
    assert!(!covers(&[ga], "\u{AC01}"));
    // An archaic leading consonant, which composes into no syllable.
    assert!(!covers(&[ga], "\u{1113}\u{1161}"));
    assert!(covers(&['\u{1113}', '\u{1161}'], "\u{1113}\u{1161}"));
}

/// A font in which harfrust draws a cluster with a `.notdef` is refused, and
/// the cluster falls back, as Chrome's does, though a whole form of the
/// cluster is in the font.
///
/// - Vietnamese with the tone typed after a precomposed vowel: no pair
///   composes `ê` and U+0323. A font with `ệ` and neither mark, such as
///   Noto Sans CJK and Source Han, is left with U+0323. A font with the
///   marks and `ẹ` is not: harfrust decomposes `ê` to reach `ệ` through
///   `ẹ`.
/// - A composite missing on the way: `Ở` from `O` U+031B U+0309 and `Ὅ`
///   from `Ο` U+0314 U+0301, in fonts that have them and not `Ơ` or `Ὁ`.
#[test]
fn what_the_whole_forms_overclaimed_falls_back() {
    assert!(!covers(&['\u{EA}', '\u{1EC7}'], "\u{EA}\u{323}"));
    assert!(covers(
        &['e', '\u{EA}', '\u{302}', '\u{1EB9}', '\u{1EC7}'],
        "\u{EA}\u{323}"
    ));
    assert!(!covers(&['O', '\u{1ECE}', '\u{1EDE}'], "O\u{31B}\u{309}"));
    assert!(!covers(
        &['\u{39F}', '\u{301}', '\u{1F4D}'],
        "\u{39F}\u{314}\u{301}"
    ));

    let mut fixture = fixture_with(&[
        named(
            "Test Viet",
            &['\u{EA}', '\u{1EC7}', '\u{39F}', '\u{301}', '\u{1F4D}'],
        ),
        named(
            "Test Marks Too",
            &['\u{EA}', '\u{323}', '\u{39F}', '\u{314}', '\u{301}'],
        ),
    ]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Viet"),
        FontFamilyName::named("Test Marks Too"),
    ];
    fixture.span(&mut layout, &families_style(&listed), "\u{EA}\u{323}");
    assert_eq!(fixture.families(&layout), ["Test Marks Too"]);
    fixture.span(
        &mut layout,
        &families_style(&listed),
        "\u{39F}\u{314}\u{301}",
    );
    assert_eq!(fixture.families(&layout), ["Test Marks Too"]);
}

/// Where only harfrust's normalizer draws a cluster in the font asked for,
/// that font keeps it, and no later font takes it mid-word.
#[test]
fn the_font_asked_for_keeps_what_harfrust_draws_in_it() {
    let mut fixture = fixture_with(&[
        named("Test Pinyin", &['l', '\u{FC}', '\u{301}']),
        named("Test Precomposed", &['l', '\u{1D8}']),
    ]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Pinyin"),
        FontFamilyName::named("Test Precomposed"),
    ];
    fixture.span(&mut layout, &families_style(&listed), "l\u{1D8}");
    assert_eq!(fixture.families(&layout), ["Test Pinyin", "Test Pinyin"]);
}

/// Roboto, from parley's development fonts in the cargo git checkout, is
/// covered as harfrust draws it; the test is skipped without the checkout.
///
/// - It has `ü` and U+0301 and not `ǘ` or U+0308, so harfrust draws the
///   pinyin tones, which no whole form covers.
/// - It has `Ὅ` and not `Ὁ` or U+0314, so harfrust leaves `Ο` U+0314
///   U+0301 with a `.notdef`, though a whole form covers it.
#[test]
fn a_real_font_is_covered_as_harfrust_draws_it() {
    let Some(bytes) = roboto() else {
        eprintln!("parley_dev's Roboto is not in the cargo git checkouts; skipped");
        return;
    };
    let font = fontwich::Font::from_data(bytes.clone(), 0);
    for (text, drawn) in [
        ("\u{1D8}", true),
        ("\u{1DC}", true),
        ("\u{1D7}", true),
        ("\u{1DB}", true),
        ("u\u{308}\u{301}", true),
        ("\u{39F}\u{314}\u{301}", false),
    ] {
        assert_eq!(harfrust_draws(&bytes, text), drawn, "{text:?}");
        assert_eq!(coverage::covers(&font, text, &mut None), drawn, "{text:?}");
        assert_eq!(covered_in_a_whole_form(&font, text), !drawn, "{text:?}");
    }
}

/// Returns parley_dev's Roboto from the cargo git checkout of parley, if
/// there is one.
fn roboto() -> Option<Vec<u8>> {
    let home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(|home| Path::new(&home).join(".cargo"))
    })?;
    let checkouts = home.join("git").join("checkouts");
    for repository in fs::read_dir(checkouts).ok()?.flatten() {
        if !repository
            .file_name()
            .to_string_lossy()
            .starts_with("parley-")
        {
            continue;
        }
        for revision in fs::read_dir(repository.path()).ok()?.flatten() {
            let file = revision
                .path()
                .join("parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf");
            if let Ok(bytes) = fs::read(file) {
                return Some(bytes);
            }
        }
    }
    None
}

/// The characters the random fonts of [`coverage_is_what_harfrust_draws`]
/// map from, and the clusters it asks about, all set by harfrust's default
/// shaper: Latin and Greek letters in their composed and decomposed forms
/// and the pieces between, spaces, hyphens, a joiner and a selector.
const LATIN_POOL: &[char] = &[
    'e', '\u{E9}', '\u{301}', '\u{302}', '\u{323}', '\u{1EB9}', '\u{1EC7}', '\u{EA}', '\u{1EBF}',
    'u', '\u{FC}', '\u{1D8}', '\u{308}', 'c', '\u{E7}', '\u{327}', '\u{1E09}', 'A', '\u{C5}',
    '\u{30A}', '\u{212B}', ' ', '\u{2002}', '\u{2010}', '\u{3B1}', '\u{3AC}', '\u{1F71}',
    '\u{345}', '\u{1FB4}', '\u{313}', '\u{1F00}', '\u{1F04}', '\u{34F}', '\u{FE00}',
];
const LATIN_CLUSTERS: &[&str] = &[
    "\u{E9}",
    "e\u{301}",
    "\u{EA}\u{301}",
    "\u{1EBF}",
    "e\u{302}\u{301}",
    "\u{1EC7}",
    "\u{1EB9}\u{302}",
    "\u{EA}\u{323}",
    "e\u{323}\u{302}",
    "e\u{302}\u{323}",
    "\u{1D8}",
    "u\u{308}\u{301}",
    "\u{FC}\u{301}",
    "u\u{301}\u{308}",
    "\u{1E09}",
    "c\u{327}\u{301}",
    "\u{E7}\u{301}",
    "c\u{301}\u{327}",
    "\u{C5}",
    "\u{212B}",
    "A\u{30A}",
    "\u{3AC}",
    "\u{1F71}",
    "\u{3B1}\u{301}",
    "\u{1FB4}",
    "\u{3B1}\u{301}\u{345}",
    "\u{3AC}\u{345}",
    "\u{3B1}\u{345}\u{301}",
    "\u{1F04}",
    "\u{3B1}\u{313}\u{301}",
    "\u{1F00}\u{301}",
    "\u{2000}",
    "\u{2002}",
    "\u{2003}",
    "\u{A0}",
    "\u{2011}",
    "e\u{34F}\u{301}",
    "e\u{301}\u{FE00}",
    "\u{E9}\u{FE00}",
    "e\u{200D}",
];

/// The same for harfrust's Hangul shaper: jamo old and modern, a filler,
/// syllables and a tone mark.
const HANGUL_POOL: &[char] = &[
    '\u{1100}', '\u{1161}', '\u{11A8}', '\u{AC00}', '\u{AC01}', '\u{1113}', '\u{11C3}', '\u{1160}',
    '\u{302E}', ' ',
];
const HANGUL_CLUSTERS: &[&str] = &[
    "\u{AC00}",
    "\u{AC01}",
    "\u{1100}\u{1161}",
    "\u{1100}\u{1161}\u{11A8}",
    "\u{AC00}\u{11A8}",
    "\u{AC00}\u{11C3}",
    "\u{AC01}\u{11C3}",
    "\u{1113}\u{1161}",
    "\u{1100}\u{1160}",
    "\u{AC00}\u{302E}",
    "\u{1100}",
];

/// Returns whether harfrust shapes `text` in the font `bytes` hold with no
/// `.notdef`, with the buffer set up as shaping sets it up.
///
/// The script is Hangul where `text` starts with a jamo or a syllable,
/// Greek where it starts with a Greek letter, and Latin otherwise.
fn harfrust_draws(bytes: &[u8], text: &str) -> bool {
    let script = match text.chars().next().map_or(0, u32::from) {
        0x1100..=0x11FF | 0xAC00..=0xD7A3 => Script::HANGUL,
        0x370..=0x3FF | 0x1F00..=0x1FFF => Script::GREEK,
        _ => Script::LATIN,
    };
    let font = harfrust::Font::new(bytes.to_vec(), 0).expect("a font");
    let shaper = harfrust::ShaperFont::new(&font);
    let mut buffer = harfrust::Buffer::new();
    buffer.set_direction(harfrust::Direction::LeftToRight);
    buffer.set_script(Some(script));
    buffer.set_cluster_level(harfrust::ClusterLevel::MonotoneCharacters);
    buffer.set_flags(harfrust::BufferFlags::BEGINNING_OF_TEXT);
    buffer.push_str(text);
    harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new()).expect("shaped");
    buffer.glyph_infos().iter().all(|glyph| glyph.glyph_id != 0)
}

/// Returns whether `font` covers `text` in a whole form: as written, fully
/// decomposed or fully composed, each with the shaper's substitutions, and
/// with Unicode's default-ignorables standing in for HarfBuzz's.
///
/// This is the baseline that replaying harfrust's normalizer improves on.
fn covered_in_a_whole_form(font: &fontwich::Font, text: &str) -> bool {
    fn draws(charset: &fontwich::Charset, mut chars: impl Iterator<Item = char>) -> bool {
        chars.all(|ch| {
            coverage::maps(charset, ch, &mut None)
                || coverage::faked_from(ch).map_or_else(
                    || unicode::rare_props(ch).is_default_ignorable(),
                    |from| coverage::maps(charset, from, &mut None),
                )
        })
    }
    let charset = font.charset();
    let nfd = DecomposingNormalizerBorrowed::new_nfd();
    let nfc = ComposingNormalizerBorrowed::new_nfc();
    draws(charset, text.chars())
        || draws(charset, nfd.normalize_iter(text.chars()))
        || draws(charset, nfc.normalize_iter(text.chars()))
}

/// Over random fonts, each mapping a random subset of a pool, coverage says
/// a font covers one of the pool's clusters exactly where harfrust draws it
/// with no `.notdef`. Some clusters' whole forms disagree with harfrust, and
/// coverage follows harfrust there.
#[test]
fn coverage_is_what_harfrust_draws() {
    struct Lcg(u64);
    impl Lcg {
        fn coin(&mut self) -> bool {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 63 == 1
        }
    }
    let mut random = Lcg(0xC0FF_EE00_0B5E_55ED);
    let (mut asked, mut covered, mut changed) = (0, 0, Vec::new());
    for (pool, clusters, fonts) in [
        (LATIN_POOL, LATIN_CLUSTERS, 400),
        (HANGUL_POOL, HANGUL_CLUSTERS, 200),
    ] {
        for _ in 0..fonts {
            let chars: Vec<char> = pool.iter().copied().filter(|_| random.coin()).collect();
            let bytes = mapping(&chars).build();
            let font = fontwich::Font::from_data(bytes.clone(), 0);
            for &text in clusters {
                let draws = harfrust_draws(&bytes, text);
                let says = coverage::covers(&font, text, &mut None);
                assert_eq!(says, draws, "{text:?} in a font of {chars:?}");
                asked += 1;
                covered += usize::from(draws);
                if covered_in_a_whole_form(&font, text) != draws {
                    changed.push((text, chars.clone(), draws));
                }
            }
        }
    }
    eprintln!(
        "{asked} clusters asked of random fonts, {covered} covered; {} answered otherwise by \
         the whole forms, {} of them now covered",
        changed.len(),
        changed.iter().filter(|(_, _, draws)| *draws).count()
    );
    let mut texts: Vec<&str> = changed.iter().map(|(text, _, _)| *text).collect();
    texts.sort_unstable();
    texts.dedup();
    eprintln!("clusters whose answer changed: {texts:?}");
    assert!(
        covered > asked / 10 && covered < asked * 9 / 10,
        "{covered} of {asked}"
    );
    assert!(!changed.is_empty());
}
