//! The case mapping against ICU4X's, icu_casemap 2.3:
//! - every character alone, in the root and each tailored language;
//! - a random corpus of the sequences the conditional mappings turn on;
//! - where Chrome differs from ICU4X: no Georgian Mtavruli in uppercase, no
//!   Armenian tailoring, and ICU4C's Dutch `IJ`.

use alloc::string::String;
use alloc::vec::Vec;

use icu_casemap::options::{LeadingAdjustment, TitlecaseOptions, TrailingCase};
use icu_casemap::{CaseMapper, CaseMapperBorrowed};
use icu_locale_core::{LanguageIdentifier, langid};

use crate::unicode::{CaseLanguage, lowercase, titlecase, uppercase};

const ICU: CaseMapperBorrowed<'static> = CaseMapper::new();

/// Every rule set, with the language ICU4X tailors it for.
fn languages() -> [(CaseLanguage, LanguageIdentifier); 5] {
    [
        (CaseLanguage::Root, LanguageIdentifier::UNKNOWN),
        (CaseLanguage::Turkic, langid!("tr")),
        (CaseLanguage::Lithuanian, langid!("lt")),
        (CaseLanguage::Greek, langid!("el")),
        (CaseLanguage::Dutch, langid!("nl")),
    ]
}

fn title_options() -> TitlecaseOptions {
    let mut options = TitlecaseOptions::default();
    options.leading_adjustment = Some(LeadingAdjustment::None);
    options.trailing_case = Some(TrailingCase::Unchanged);
    options
}

fn ours(
    text: &str,
    map: fn(&str, CaseLanguage, &mut dyn FnMut(char)),
    language: CaseLanguage,
) -> String {
    let mut out = String::new();
    map(text, language, &mut |ch| out.push(ch));
    out
}

fn upper(text: &str, language: CaseLanguage) -> String {
    ours(
        text,
        |text, language, push| uppercase(text, language, &mut |ch| push(ch)),
        language,
    )
}

fn lower(text: &str, language: CaseLanguage) -> String {
    ours(
        text,
        |text, language, push| lowercase(text, language, &mut |ch| push(ch)),
        language,
    )
}

fn title(text: &str, language: CaseLanguage) -> String {
    ours(
        text,
        |text, language, push| titlecase(text, language, &mut |ch| push(ch)),
        language,
    )
}

/// ICU4X's uppercase with Mtavruli lowered, as Chrome's
/// `DisableNewGeorgianCapitalLetters` lowers it.
fn icu_upper(text: &str, langid: &LanguageIdentifier) -> String {
    ICU.uppercase_to_string(text, langid)
        .chars()
        .map(|ch| match ch {
            '\u{1C90}'..='\u{1CBF}' => char::from_u32(u32::from(ch) - 0xBC0).unwrap_or(ch),
            _ => ch,
        })
        .collect()
}

#[test]
fn every_character_alone_maps_as_icu_does() {
    let options = title_options();
    for (language, langid) in languages() {
        for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            let mut buffer = [0; 4];
            let one: &str = ch.encode_utf8(&mut buffer);
            let cp = u32::from(ch);
            assert_eq!(
                upper(one, language),
                icu_upper(one, &langid),
                "U+{cp:04X} {language:?}"
            );
            assert_eq!(
                lower(one, language),
                ICU.lowercase_to_string(one, &langid),
                "U+{cp:04X} {language:?}"
            );
            assert_eq!(
                title(one, language),
                ICU.titlecase_segment_with_only_case_data_to_string(one, &langid, options),
                "U+{cp:04X} {language:?}"
            );
        }
    }
}

/// The characters the conditional mappings read: `i`, `I`, `J` and their
/// accented forms, the combining marks above and below, sigmas, Greek
/// letters and diacritics, case-ignorable and uncased characters.
const ALPHABET: &[char] = &[
    'a',
    'A',
    'i',
    'I',
    'j',
    'J',
    'x',
    '\u{12E}',
    '\u{12F}',
    '\u{CC}',
    '\u{CD}',
    '\u{128}',
    '\u{130}',
    '\u{131}',
    '\u{1E2D}',
    '\u{300}',
    '\u{301}',
    '\u{303}',
    '\u{307}',
    '\u{308}',
    '\u{316}',
    '\u{328}',
    '\u{3A3}',
    '\u{3C3}',
    '\u{3C2}',
    '\u{391}',
    '\u{3AC}',
    '\u{397}',
    '\u{3AE}',
    '\u{3B7}',
    '\u{399}',
    '\u{3B9}',
    '\u{3CA}',
    '\u{390}',
    '\u{3A5}',
    '\u{3C5}',
    '\u{3CD}',
    '\u{3B0}',
    '\u{1FB3}',
    '\u{1F00}',
    '\u{1F04}',
    '\u{1FBC}',
    '\u{313}',
    '\u{314}',
    '\u{342}',
    '\u{343}',
    '\u{344}',
    '\u{345}',
    '\u{2126}',
    '\u{3D2}',
    '\u{3D3}',
    ' ',
    '\'',
    '.',
    '\u{AD}',
    '\u{2BC}',
    '1',
    '\u{DF}',
    '\u{1C5}',
    '\u{1C6}',
    '\u{FB01}',
    '\u{149}',
    '\u{587}',
    '\u{10400}',
    '\u{10D0}',
    '\u{1C90}',
];

/// Random strings of up to eight characters of [`ALPHABET`], the same ones
/// each run.
fn corpus() -> impl Iterator<Item = String> {
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move |below: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % below as u64) as usize
    };
    (0..40_000).map(move |_| {
        let len = 1 + next(8);
        (0..len).map(|_| ALPHABET[next(ALPHABET.len())]).collect()
    })
}

#[test]
fn sequences_map_as_icu_does() {
    let options = title_options();
    let texts: Vec<String> = corpus().collect();
    for (language, langid) in languages() {
        for text in &texts {
            // ICU4X uppercases Greek otherwise than ICU4C's `GreekUpper`
            // where marks pile up, which the Chrome pins below hold.
            if language != CaseLanguage::Greek {
                assert_eq!(
                    upper(text, language),
                    icu_upper(text, &langid),
                    "{text:?} {language:?}"
                );
            }
            assert_eq!(
                lower(text, language),
                ICU.lowercase_to_string(text, &langid),
                "{text:?} {language:?}"
            );
            // ICU4X finds the Dutch `IJ` otherwise than ICU4C, which the
            // Chrome pins below hold.
            if language != CaseLanguage::Dutch {
                assert_eq!(
                    title(text, language),
                    ICU.titlecase_segment_with_only_case_data_to_string(text, &langid, options),
                    "{text:?} {language:?}"
                );
            }
        }
    }
}

/// Chrome 155 uppercases with ICU4C's root mapping in every language but
/// Turkish, Azerbaijani, Lithuanian and Greek, so `և` is `ԵՒ` in Armenian
/// too, and it lowers every Georgian capital it makes, or finds.
#[test]
fn uppercase_follows_chrome_where_icu4x_does_not() {
    assert_eq!(upper("\u{587}", CaseLanguage::Root), "\u{535}\u{552}");
    assert_eq!(
        upper("\u{10E5}\u{10D0} \u{1CA5}\u{1C90}", CaseLanguage::Root),
        "\u{10E5}\u{10D0} \u{10E5}\u{10D0}"
    );
    assert_eq!(
        title("\u{10E5}\u{10D0}", CaseLanguage::Root),
        "\u{10E5}\u{10D0}"
    );
    assert_eq!(title("\u{587}", CaseLanguage::Root), "\u{535}\u{582}");
}

/// Chrome 155 uppercases Greek by ICU4C's `GreekUpper`: accents and
/// breathings dropped, a dialytika kept or gained, a lone `ή` keeping its
/// accent, and an iota subscript a capital iota. A dialytika on a consonant
/// stays, where ICU4X drops it.
#[test]
fn greek_uppercases_as_chrome_does() {
    let greek = |text| upper(text, CaseLanguage::Greek);
    assert_eq!(
        greek(
            "άλφα ή Ή Άυλος ᾳ ῥ προϊόν ευρώ ά\u{301} ἀι ἄι αΐ ή, ή. η\u{301} ᾷ ΐ ϊ\u{301} \
             ω\u{342}\u{345} ς"
        ),
        "ΑΛΦΑ Ή Ή ΑΫΛΟΣ ΑΙ Ρ ΠΡΟΪΟΝ ΕΥΡΩ Α ΑΙ ΑΪ ΑΪ Ή, Ή. Η\u{301} ΑΙ Ϊ Ϊ ΩΙ Σ"
    );
    assert_eq!(greek("ϝ\u{344}"), "Ϝ\u{308}");
    assert_eq!(greek("ς\u{308}"), "Σ\u{308}");
    assert_eq!(greek("ῤ\u{308}"), "Ρ\u{308}");
    assert_eq!(greek("\u{303}η\u{344}"), "\u{303}Η\u{308}\u{301}");
    assert_eq!(greek("\u{303}Ͱ\u{344}"), "\u{303}Ͱ\u{308}");
    assert_eq!(upper("άλφα", CaseLanguage::Root), "ΆΛΦΑ");
}

/// Chrome 155's full mappings and conditional ones, through ICU4C: the
/// root's, then Turkish and Lithuanian `i` and dots, and the final sigma.
#[test]
fn the_mappings_are_chromes() {
    use CaseLanguage::{Lithuanian, Root, Turkic};
    assert_eq!(
        upper(
            "straße ﬁ ﬀ ﬃ ﬆ ŉ ǰ ΐ ΰ և ᾳ ῳ ᾀ ᾷ ǆ ǅ ı i ſ µ ÿ ꭰꭱ ꞵ ɐ",
            Root
        ),
        "STRASSE FI FF FFI ST ʼN J\u{30C} Ι\u{308}\u{301} Υ\u{308}\u{301} ԵՒ ΑΙ ΩΙ ἈΙ Α\u{342}Ι Ǆ Ǆ I I S Μ Ÿ ᎠᎡ \
         Ꞵ Ɐ"
    );
    assert_eq!(
        lower("ΣΑΣ ΑΣ. Σ ΑΣ\u{301} Α\u{AD}Σ ΑΣA İ I ǅ Ǆ ẞ K Ω Ꭰ ᏸ", Root),
        "σας ας. σ ας\u{301} α\u{AD}ς ασa i\u{307} i ǆ ǆ ß k ω ꭰ ᏸ"
    );
    assert_eq!(
        upper("i ı istanbul i\u{307}", Turkic),
        "İ I İSTANBUL İ\u{307}"
    );
    assert_eq!(
        lower("IŞIK İ I\u{307} I\u{301}\u{307} IA", Turkic),
        "ışık i i ı\u{301}\u{307} ıa"
    );
    assert_eq!(
        upper(
            "i\u{307} į\u{307} i\u{301}\u{307} j\u{307} i\u{307}\u{301} a\u{307}",
            Lithuanian
        ),
        "I Į I\u{301}\u{307} J I\u{301} A\u{307}"
    );
    assert_eq!(upper("i\u{307} į\u{307}", Root), "I\u{307} Į\u{307}");
    assert_eq!(
        lower(
            "I\u{301} Ì Í Ĩ J\u{301} Į\u{301} I I\u{328}\u{301} I\u{316}\u{301} Ì\u{301}",
            Lithuanian
        ),
        "i\u{307}\u{301} i\u{307}\u{300} i\u{307}\u{301} i\u{307}\u{303} j\u{307}\u{301} \
         į\u{307}\u{301} i i\u{307}\u{328}\u{301} i\u{307}\u{316}\u{301} i\u{307}\u{300}\u{301}"
    );
    assert_eq!(title("istanbul", Turkic), "İstanbul");
    assert_eq!(title("ǆemal", Root), "ǅemal");
    assert_eq!(title("ﬁsh", Root), "Fish");
    assert_eq!(title("ßa", Root), "Ssa");
    assert_eq!(title("ᾳa", Root), "ᾼa");
    assert_eq!(title("ΐa", CaseLanguage::Greek), "Ι\u{308}\u{301}a");
}

/// Chrome 155's capitalize with `ICUCapitalization` titlecases a Dutch
/// `IJ` by ICU4C's `maybeTitleDutchIJ`: an acute on both or on neither, and
/// no other mark after.
#[test]
fn dutch_titlecases_ij_as_icu4c_does() {
    let dutch = |text| title(text, CaseLanguage::Dutch);
    assert_eq!(dutch("ijsland"), "IJsland");
    assert_eq!(dutch("Ijs"), "IJs");
    assert_eq!(dutch("IJs"), "IJs");
    assert_eq!(dutch("ij"), "IJ");
    assert_eq!(dutch("\u{ED}j\u{301}s"), "\u{CD}J\u{301}s");
    assert_eq!(dutch("i\u{301}j\u{301}s"), "I\u{301}J\u{301}s");
    assert_eq!(dutch("ij\u{301}s"), "Ij\u{301}s");
    assert_eq!(
        dutch("i\u{301}j\u{301}\u{301}s"),
        "I\u{301}j\u{301}\u{301}s"
    );
    assert_eq!(title("ijsland", CaseLanguage::Root), "Ijsland");
}
