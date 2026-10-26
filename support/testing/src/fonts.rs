//! Fonts built in memory, and the collections the crate's tests set text in.
//!
//! A test written against a font the machine happens to have is a test of
//! the machine. Ahem is vendored for that reason; beside it these are built
//! from a description, carrying the tables the stages read and nothing else:
//! a Unicode cmap for coverage, `name` for the family, `head`, `hhea`, `OS/2`
//! and `post` for the metrics, `maxp` and `hmtx` so that a shaper can read
//! them, a `COLR` header where the font is to count as a color font, `GSUB`
//! and `GPOS` where the shaper is to substitute or move something:
//! ligatures, Arabic joining forms, a character drawn as two glyphs, a glyph
//! drawn raised, a kerning pair, a form one language system asks for, a
//! feature's alternate glyphs, directly or only through a contextual lookup;
//! `fvar` and `HVAR` where the font varies,
//! its axes moving every advance; `trak`, with the `STAT` HarfBuzz asks
//! for beside it, where the font tracks by the point size it is set at; and
//! `vhea`, `vmtx` and `VORG` where it has vertical metrics. A font of more
//! than 65,535 glyphs has `MAXP`, `HHEA` and `HMTX` beside `maxp`, `hhea`
//! and `hmtx`, and its alternates past glyph 65,535 are a 24-bit single
//! substitution over a 24-bit coverage.
//! Deterministic, and in every build: no file, no licence.
//!
//! Shared by the crate's unit tests and its integration tests, so it names
//! nothing of the crate's, only fontwich's, and by a crate that is `no_std`
//! when its tests run without default features, so it names `alloc` rather
//! than `std`.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Reverse;

use fontwich::{
    Collection, FallbackKey, FallbackOverride, Family, LayerBuilder, Presentation, Role, Script,
};

/// Ahem, from the web platform tests: see `support/testing/fonts`.
pub const AHEM: &[u8] = include_bytes!("../fonts/Ahem.ttf");

/// The variation-sequence probe's fonts, VsA to VsF, by family: a few
/// rectangles each, whose advances say which font drew a cluster and whether
/// it drew a sequence's own glyph, which Chrome 153 was measured with. See
/// `support/testing/fonts`.
pub const VARIATION_FONTS: [(&str, &[u8]); 6] = [
    ("VsA", include_bytes!("../fonts/VsA.ttf")),
    ("VsB", include_bytes!("../fonts/VsB.ttf")),
    ("VsC", include_bytes!("../fonts/VsC.ttf")),
    ("VsD", include_bytes!("../fonts/VsD.ttf")),
    ("VsE", include_bytes!("../fonts/VsE.ttf")),
    ("VsF", include_bytes!("../fonts/VsF.ttf")),
];

/// Ahem, then `fonts`, in one application layer whose fallback is
/// `fallback`: what a test's context is over.
pub fn collection(fonts: &[TestFont], fallback: TestFallback) -> Collection {
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(AHEM).is_ok());
    for font in fonts {
        assert!(
            layer.add_data(font.build()).is_ok(),
            "{} is a font",
            font.family
        );
    }
    layer.set_fallback_override(fallback);
    Collection::new().with_layer(layer.snapshot())
}

/// A test's own fallback: families named for every key, ahead of
/// everything, and families for Han and for emoji alone.
#[derive(Clone, Default, Debug)]
pub struct TestFallback {
    /// Every key's, first: a generic's family too.
    every: Vec<String>,
    /// A Han key's, after those.
    han: Vec<String>,
    /// The color emoji key's.
    emoji: Vec<String>,
}

impl TestFallback {
    /// A fallback naming nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `family` for every key: a generic resolves to the first.
    #[must_use]
    pub fn family(mut self, family: &str) -> Self {
        self.every.push(String::from(family));
        self
    }

    /// Adds `family` for Han, in any tradition, after the families for
    /// every key.
    #[must_use]
    pub fn han_family(mut self, family: &str) -> Self {
        self.han.push(String::from(family));
        self
    }

    /// Adds `family` for emoji in emoji presentation.
    #[must_use]
    pub fn emoji_family(mut self, family: &str) -> Self {
        self.emoji.push(String::from(family));
        self
    }
}

impl FallbackOverride for TestFallback {
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
        let mut add = |names: &[String]| {
            out.extend(
                names
                    .iter()
                    .filter_map(|name| collection.fallback_family(name)),
            );
        };
        match key.presentation() {
            Some(Presentation::Emoji) => add(&self.emoji),
            Some(_) => {}
            None => {
                add(&self.every);
                if key.script() == Some(Script::from_bytes(*b"Hani")) {
                    add(&self.han);
                }
            }
        }
    }
}

/// The fallback most tests set text with: Ahem at the head of every key's
/// families.
pub fn ahem_fallback() -> TestFallback {
    TestFallback::new().family("Ahem")
}

/// Ahem at the head of every key's families, and `family` among Han's.
pub fn han_fallback(family: &str) -> TestFallback {
    ahem_fallback().han_family(family)
}

/// ASCII with `fi`, `fl` and `ffi` ligatures, three quarters of an em, and
/// `AV` kerned by a tenth of an em: every other glyph half an em.
pub fn latin() -> TestFont {
    let mut font = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    font.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    font.kerning = vec![('A', 'V', -100)];
    font
}

/// A Han font: ideographs, kana and CJK punctuation, and digits and a space,
/// which it must not take from the font asked for first.
pub fn han() -> TestFont {
    TestFont::new(
        "Test Han",
        &[
            (0x20, 0x20),
            (0x30, 0x39),
            (0x3000, 0x30FF),
            (0x4E00, 0x9FFF),
        ],
    )
}

/// The joining forms a [`TestFont`] gives each of its joining characters, in
/// the order their glyphs follow one another.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Form {
    Initial,
    Medial,
    Final,
}

/// A variation axis of a [`TestFont`], in user units: its range and default,
/// and how far it moves every glyph's advance, in font units, at its
/// maximum, from nothing at its default.
#[derive(Copy, Clone, Debug)]
pub struct TestAxis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
    pub delta: i16,
}

/// A feature that substitutes: its tag, and each character it gives an
/// alternate glyph of its own, with that glyph's advance in units.
pub type Alternates = ([u8; 4], Vec<(char, u16)>);

/// The vertical metrics of a [`TestFont`], in `vhea` and `vmtx`, and in
/// `VORG` where it has an origin table: every glyph's vertical advance and
/// top side bearing, in units, but those of the characters listed.
#[derive(Clone, Debug)]
pub struct TestVertical {
    pub advance: u16,
    pub top_side_bearing: i16,
    /// The advances of the characters that have another.
    pub advances: Vec<(char, u16)>,
    /// `VORG`: the default vertical origin, and the glyphs whose origin is
    /// another.
    pub origins: Option<(i16, Vec<(char, i16)>)>,
}

/// A font to build: its family, the characters it maps, and its metrics in
/// units of a 1000-unit em unless said otherwise.
///
/// Glyph 0 is `.notdef`, then one glyph per character mapped, in the order of
/// `ranges`, then the glyphs the layout tables make, in this order: one per
/// ligature, three per joining character (initial, medial, final), one per
/// split character, one for the localized character, one per alternate,
/// feature by feature, and one per contextual alternate, feature by feature.
#[derive(Clone, Debug)]
pub struct TestFont {
    pub family: String,
    /// Inclusive ranges of the characters it maps, ascending.
    pub ranges: Vec<(u32, u32)>,
    pub upem: u16,
    /// `usWinAscent`, `usWinDescent`.
    pub win: (u16, u16),
    /// `hhea` ascender, descender (negative) and line gap.
    pub hhea: (i16, i16, i16),
    /// `OS/2` typographic ascender, descender (negative) and line gap.
    pub typo: (i16, i16, i16),
    /// `OS/2` sets `USE_TYPO_METRICS`.
    pub use_typo: bool,
    pub x_height: i16,
    pub cap_height: i16,
    /// It has a `COLR` table, and so draws in color.
    pub color: bool,
    /// Its `head`, `hhea` and `OS/2` are cut short, so no metric in them can
    /// be read, though its cmap and name still can.
    pub broken: bool,
    /// The advances of characters that are not half an em wide, in units.
    pub advances: Vec<(char, u16)>,
    /// `liga`: each sequence of characters becomes one glyph of its own,
    /// three quarters of an em wide.
    pub ligatures: Vec<Vec<char>>,
    /// `init`, `medi` and `fina`: each of these characters has an initial,
    /// a medial and a final form, glyphs of their own half an em wide.
    pub joining: Vec<char>,
    /// `ccmp`: each of these characters becomes two glyphs, its own and one a
    /// quarter of an em wide after it.
    pub splits: Vec<char>,
    /// `kern`, in `GPOS`: each character's glyph drawn this many units up.
    pub raised: Vec<(char, i16)>,
    /// `kern`, in `GPOS`: each pair's first glyph advances this many units
    /// more before the second.
    pub kerning: Vec<(char, char, i16)>,
    /// `locl` under this language system of `latn`: the character takes a
    /// glyph of its own, half an em wide.
    pub localized: Option<([u8; 4], char)>,
    /// The script tags `GSUB` and `GPOS` list, ascending, each with every
    /// feature in its default language system: `DFLT` and `latn` unless set.
    pub scripts: Vec<[u8; 4]>,
    /// `GSUB` and `GPOS` are there but hold offsets past their own ends, so
    /// no lookup in them can be read.
    pub broken_layout: bool,
    /// Features that substitute: under each tag, each character takes an
    /// alternate glyph of its own with this advance, in units.
    pub alternates: Vec<Alternates>,
    /// Features that substitute only through a contextual lookup: under each
    /// tag, a context of the one glyph (`GSUB` type 5, format 3) applies a
    /// single substitution no feature names, and each character takes an
    /// alternate glyph of its own with this advance, in units.
    pub contextual: Vec<Alternates>,
    /// `OS/2`'s weight class.
    pub weight: u16,
    /// `OS/2`'s superscript and subscript sizes, in units.
    pub script_sizes: (i16, i16),
    /// The axes it varies along, in `fvar`, each moving every advance through
    /// `HVAR`.
    pub axes: Vec<TestAxis>,
    /// `halt`, in `GPOS`: each character's glyph moved this many units
    /// along the line and its advance changed by this many, as a Japanese
    /// font sets its punctuation in half its em: an opening mark
    /// `(-500, -500)`, a closing one `(0, -500)`.
    pub halt: Vec<(char, i16, i16)>,
    /// `chws`, in `GPOS`: each pair's first glyph advances this many units
    /// more before the second, as a font that collapses adjacent
    /// punctuation itself does.
    pub chws: Vec<(char, char, i16)>,
    /// `trak`'s normal track, with the `STAT` table HarfBuzz applies it
    /// beside: at each point size, ascending, how many units each grapheme's
    /// advance gains, read between them by the point size the shaper is
    /// told, as San Francisco tracks.
    pub tracking: Vec<(f32, i16)>,
    /// `vhea`, `vmtx` and `VORG`, where it has vertical metrics.
    pub vertical: Option<TestVertical>,
    /// `vhal`, in `GPOS`: each character's glyph moved this many units up
    /// and its vertical advance changed by this many, as a Japanese font
    /// sets its punctuation in half its em down a vertical line: an opening
    /// mark `(500, -500)`, a closing one `(0, -500)`, `GPOS`'s y growing up
    /// and a vertical advance shortened by a negative one.
    pub vhal: Vec<(char, i16, i16)>,
}

impl TestFont {
    /// A font named `family` mapping `ranges`, with a 1000-unit em, win and
    /// `hhea` lines of 800 above and 200 below with no gap, typographic ones
    /// of 700 and 300 with a gap of 100 that it does not ask for, and an
    /// x-height of 500 and cap height of 700. Every glyph is half an em wide,
    /// and nothing is substituted or positioned.
    pub fn new(family: &str, ranges: &[(u32, u32)]) -> Self {
        Self {
            family: family.into(),
            ranges: ranges.to_vec(),
            upem: 1000,
            win: (800, 200),
            hhea: (800, -200, 0),
            typo: (700, -300, 100),
            use_typo: false,
            x_height: 500,
            cap_height: 700,
            color: false,
            broken: false,
            advances: Vec::new(),
            ligatures: Vec::new(),
            joining: Vec::new(),
            splits: Vec::new(),
            raised: Vec::new(),
            kerning: Vec::new(),
            localized: None,
            scripts: vec![*b"DFLT", *b"latn"],
            broken_layout: false,
            alternates: Vec::new(),
            contextual: Vec::new(),
            weight: 400,
            script_sizes: (650, 650),
            axes: Vec::new(),
            halt: Vec::new(),
            chws: Vec::new(),
            tracking: Vec::new(),
            vertical: None,
            vhal: Vec::new(),
        }
    }

    /// A Japanese font's punctuation, for `text-autospace` and
    /// `text-spacing-trim`: ASCII half an em wide; a few ideographs (`漢`,
    /// `字`, `水`), the CJK and fullwidth punctuation a test sets and the
    /// ideographic space a whole em; the curly quotes half an em, as a
    /// Japanese font draws them proportional. Where `halt`, its `halt` sets
    /// each full-width mark in half its em: an opening mark moved back by
    /// half, a closing one's advance halved, a middle one each by a quarter.
    pub fn cjk(family: &str, halt: bool) -> Self {
        let wide = [
            '\u{3000}', '\u{3001}', '\u{3002}', '\u{300C}', '\u{300D}', '\u{30FB}', '\u{5B57}',
            '\u{6C34}', '\u{6F22}', '\u{FF08}', '\u{FF09}', '\u{FF0C}', '\u{FF0E}', '\u{FF1A}',
            '\u{FF1B}',
        ];
        let mut font = Self::new(
            family,
            &[
                (0x20, 0x7E),
                (0x2018, 0x201D),
                (0x3000, 0x3002),
                (0x300C, 0x300D),
                (0x30FB, 0x30FB),
                (0x5B57, 0x5B57),
                (0x6C34, 0x6C34),
                (0x6F22, 0x6F22),
                (0xFF08, 0xFF09),
                (0xFF0C, 0xFF0C),
                (0xFF0E, 0xFF0E),
                (0xFF1A, 0xFF1B),
            ],
        );
        font.advances = wide.iter().map(|&ch| (ch, 1000)).collect();
        if halt {
            font.halt = vec![
                ('\u{300C}', -500, -500),
                ('\u{FF08}', -500, -500),
                ('\u{300D}', 0, -500),
                ('\u{FF09}', 0, -500),
                ('\u{3001}', 0, -500),
                ('\u{3002}', 0, -500),
                ('\u{FF0C}', 0, -500),
                ('\u{FF0E}', 0, -500),
                ('\u{3000}', -250, -500),
                ('\u{30FB}', -250, -500),
                ('\u{FF1A}', -250, -500),
                ('\u{FF1B}', -250, -500),
            ];
        }
        font
    }

    /// How many glyphs its cmap maps to: `.notdef`, then one per character.
    fn mapped(&self) -> u32 {
        1 + self
            .ranges
            .iter()
            .map(|&(start, end)| end - start + 1)
            .sum::<u32>()
    }

    /// How many glyphs it has.
    pub fn glyph_count(&self) -> u32 {
        self.contextual_base()
            + self
                .contextual
                .iter()
                .map(|(_, chars)| chars.len() as u32)
                .sum::<u32>()
    }

    fn alternate_base(&self) -> u32 {
        self.split_base() + self.splits.len() as u32 + u32::from(self.localized.is_some())
    }

    fn contextual_base(&self) -> u32 {
        self.alternate_base()
            + self
                .alternates
                .iter()
                .map(|(_, chars)| chars.len() as u32)
                .sum::<u32>()
    }

    /// The glyph `ch` takes under `tag` among `features`, whose glyphs start
    /// at `base`, or 0 where it takes none.
    fn feature_glyph(features: &[Alternates], base: u32, tag: [u8; 4], ch: char) -> u32 {
        let mut glyph = base;
        for (feature, chars) in features {
            for &(c, _) in chars {
                if *feature == tag && c == ch {
                    return glyph;
                }
                glyph += 1;
            }
        }
        0
    }

    /// The alternate glyph feature `tag` gives `ch`, or 0 where it gives none.
    pub fn alternate_glyph(&self, tag: [u8; 4], ch: char) -> u32 {
        Self::feature_glyph(&self.alternates, self.alternate_base(), tag, ch)
    }

    /// The alternate glyph contextual feature `tag` gives `ch`, or 0 where it
    /// gives none.
    pub fn contextual_glyph(&self, tag: [u8; 4], ch: char) -> u32 {
        Self::feature_glyph(&self.contextual, self.contextual_base(), tag, ch)
    }

    /// The glyph `ch` maps to, or 0 where it maps none.
    pub fn glyph(&self, ch: char) -> u32 {
        let ch = ch as u32;
        let mut glyph = 1;
        for &(start, end) in &self.ranges {
            if (start..=end).contains(&ch) {
                return glyph + ch - start;
            }
            glyph += end - start + 1;
        }
        0
    }

    /// The glyph of ligature `at` in `ligatures`.
    pub fn ligature_glyph(&self, at: usize) -> u32 {
        self.mapped() + at as u32
    }

    fn joining_base(&self) -> u32 {
        self.mapped() + self.ligatures.len() as u32
    }

    /// The glyph of `ch`'s joining `form`, or 0 where it has none.
    pub fn form_glyph(&self, ch: char, form: Form) -> u32 {
        let Some(at) = self.joining.iter().position(|&c| c == ch) else {
            return 0;
        };
        self.joining_base() + 3 * at as u32 + form as u32
    }

    fn split_base(&self) -> u32 {
        self.joining_base() + 3 * self.joining.len() as u32
    }

    /// The second glyph split character `ch` becomes, or 0 where it is not
    /// one.
    pub fn split_glyph(&self, ch: char) -> u32 {
        match self.splits.iter().position(|&c| c == ch) {
            Some(at) => self.split_base() + at as u32,
            None => 0,
        }
    }

    /// The glyph the localized character takes under its language system.
    pub fn localized_glyph(&self) -> u32 {
        self.split_base() + self.splits.len() as u32
    }

    /// Each glyph's advance, in glyph order.
    fn glyph_advances(&self) -> Vec<u16> {
        let half = self.upem / 2;
        let mut advances = vec![half; self.glyph_count() as usize];
        for &(ch, advance) in &self.advances {
            let glyph = self.glyph(ch) as usize;
            if glyph != 0 {
                advances[glyph] = advance;
            }
        }
        for at in 0..self.ligatures.len() {
            advances[self.ligature_glyph(at) as usize] = self.upem / 4 * 3;
        }
        for &ch in &self.splits {
            advances[self.split_glyph(ch) as usize] = self.upem / 4;
        }
        for (tag, chars) in &self.alternates {
            for &(ch, advance) in chars {
                advances[self.alternate_glyph(*tag, ch) as usize] = advance;
            }
        }
        for (tag, chars) in &self.contextual {
            for &(ch, advance) in chars {
                advances[self.contextual_glyph(*tag, ch) as usize] = advance;
            }
        }
        advances
    }

    /// The font's bytes.
    pub fn build(&self) -> Vec<u8> {
        let count = self.glyph_count();
        let advances = self.glyph_advances();
        // Past 65,535 glyphs the 16-bit tables hold the first 65,535, and
        // `MAXP`, `HHEA` and `HMTX` the whole font.
        let glyphs = u16::try_from(count).unwrap_or(u16::MAX);
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"cmap", self.cmap()),
            (*b"head", self.head()),
            (*b"hhea", self.hhea(glyphs)),
            (*b"hmtx", hmtx(&advances[..usize::from(glyphs)])),
            (*b"maxp", maxp(glyphs)),
            (*b"name", name(&self.family)),
            (*b"OS/2", self.os2()),
            (*b"post", post(-100, 50)),
        ];
        if count > u32::from(u16::MAX) {
            let mut hhea = self.hhea(0);
            hhea.truncate(34);
            push32(&mut hhea, count);
            let mut maxp = Vec::new();
            push32(&mut maxp, 0x0000_5000);
            maxp.extend_from_slice(&count.to_be_bytes()[1..]);
            tables.push((*b"HHEA", hhea));
            tables.push((*b"HMTX", hmtx(&advances)));
            tables.push((*b"MAXP", maxp));
        }
        if self.color {
            tables.push((*b"COLR", colr()));
        }
        if !self.axes.is_empty() {
            tables.push((*b"fvar", fvar(&self.axes)));
            tables.push((*b"HVAR", hvar(&self.axes, short(count))));
        }
        if !self.tracking.is_empty() {
            tables.push((*b"trak", trak(&self.tracking)));
            tables.push((*b"STAT", stat()));
        }
        if let Some(vertical) = &self.vertical {
            tables.push((*b"vhea", vhea(short(count))));
            tables.push((*b"vmtx", self.vmtx(vertical)));
            if let Some((default, origins)) = &vertical.origins {
                let mut listed: Vec<(u16, i16)> =
                    origins.iter().map(|&(ch, y)| (self.g(ch), y)).collect();
                listed.sort_unstable();
                tables.push((*b"VORG", vorg(*default, &listed)));
            }
        }
        if self.broken_layout {
            tables.push((*b"GSUB", broken_layout()));
            tables.push((*b"GPOS", broken_layout()));
        } else {
            if let Some(gsub) = self.gsub() {
                tables.push((*b"GSUB", gsub));
            }
            if let Some(gpos) = self.gpos() {
                tables.push((*b"GPOS", gpos));
            }
        }
        if self.broken {
            for (tag, table) in &mut tables {
                if matches!(&*tag, b"head" | b"hhea" | b"OS/2") {
                    table.truncate(4);
                }
            }
        }
        sfnt(tables)
    }

    fn cmap(&self) -> Vec<u8> {
        // One Windows Unicode full-repertoire subtable, format 12, mapping
        // each range to consecutive glyphs from 1.
        let mut table = Vec::new();
        push16(&mut table, 0); // version
        push16(&mut table, 1); // one subtable
        push16(&mut table, 3); // Windows
        push16(&mut table, 10); // Unicode full repertoire
        push32(&mut table, 12); // offset
        push16(&mut table, 12); // format
        push16(&mut table, 0);
        push32(&mut table, 16 + 12 * self.ranges.len() as u32);
        push32(&mut table, 0); // language
        push32(&mut table, self.ranges.len() as u32);
        let mut glyph = 1u32;
        for &(start, end) in &self.ranges {
            push32(&mut table, start);
            push32(&mut table, end);
            push32(&mut table, glyph);
            glyph += end - start + 1;
        }
        table
    }

    fn head(&self) -> Vec<u8> {
        let mut table = vec![0u8; 54];
        table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        table[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        table[18..20].copy_from_slice(&self.upem.to_be_bytes());
        table
    }

    fn hhea(&self, metrics: u16) -> Vec<u8> {
        let mut table = vec![0u8; 36];
        table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        table[4..6].copy_from_slice(&self.hhea.0.to_be_bytes());
        table[6..8].copy_from_slice(&self.hhea.1.to_be_bytes());
        table[8..10].copy_from_slice(&self.hhea.2.to_be_bytes());
        table[18..20].copy_from_slice(&1u16.to_be_bytes()); // caret rise
        table[34..36].copy_from_slice(&metrics.to_be_bytes());
        table
    }

    fn os2(&self) -> Vec<u8> {
        let mut table = vec![0u8; 96];
        let mut set = |at: usize, bytes: [u8; 2]| table[at..at + 2].copy_from_slice(&bytes);
        set(0, 4u16.to_be_bytes()); // version
        set(4, self.weight.to_be_bytes()); // weight
        set(6, 5u16.to_be_bytes()); // width
        set(12, self.script_sizes.1.to_be_bytes()); // subscript size
        set(16, 140i16.to_be_bytes()); // subscript offset, down
        set(20, self.script_sizes.0.to_be_bytes()); // superscript size
        set(24, 480i16.to_be_bytes()); // superscript offset, up
        set(26, 50i16.to_be_bytes()); // strikeout size
        set(28, 250i16.to_be_bytes()); // strikeout position
        let selection: u16 = if self.use_typo { 0x40 | 0x80 } else { 0x40 };
        set(62, selection.to_be_bytes());
        set(68, self.typo.0.to_be_bytes());
        set(70, self.typo.1.to_be_bytes());
        set(72, self.typo.2.to_be_bytes());
        set(74, self.win.0.to_be_bytes());
        set(76, self.win.1.to_be_bytes());
        set(86, self.x_height.to_be_bytes());
        set(88, self.cap_height.to_be_bytes());
        table
    }

    /// `vmtx`: every glyph a long metric of its own.
    fn vmtx(&self, vertical: &TestVertical) -> Vec<u8> {
        let mut advances = vec![vertical.advance; self.glyph_count() as usize];
        for &(ch, advance) in &vertical.advances {
            advances[self.glyph(ch) as usize] = advance;
        }
        let mut table = Vec::new();
        for advance in advances {
            push16(&mut table, advance);
            push16(&mut table, vertical.top_side_bearing as u16);
        }
        table
    }

    /// The glyph `ch` maps to, as a layout table holds it.
    fn g(&self, ch: char) -> u16 {
        short(self.glyph(ch))
    }

    /// `GSUB`, or `None` where nothing is substituted.
    fn gsub(&self) -> Option<Vec<u8>> {
        let mut features: Vec<([u8; 4], Vec<u16>)> = Vec::new();
        let mut lookups: Vec<Vec<u8>> = Vec::new();
        // Each feature has one lookup of its own.
        let mut feature = |tag: [u8; 4], lookup: Vec<u8>| -> u16 {
            let at = features.len() as u16;
            features.push((tag, vec![lookups.len() as u16]));
            lookups.push(lookup);
            at
        };
        if !self.splits.is_empty() {
            let mut pairs: Vec<(u16, Vec<u16>)> = self
                .splits
                .iter()
                .map(|&ch| (self.g(ch), vec![self.g(ch), short(self.split_glyph(ch))]))
                .collect();
            pairs.sort();
            feature(*b"ccmp", lookup(2, multiple_subst(&pairs)));
        }
        if !self.joining.is_empty() {
            for (tag, form) in [
                (*b"fina", Form::Final),
                (*b"init", Form::Initial),
                (*b"medi", Form::Medial),
            ] {
                let mut pairs: Vec<(u16, u16)> = self
                    .joining
                    .iter()
                    .map(|&ch| (self.g(ch), short(self.form_glyph(ch, form))))
                    .collect();
                pairs.sort();
                feature(tag, lookup(1, single_subst(&pairs)));
            }
        }
        if !self.ligatures.is_empty() {
            let ligatures: Vec<(Vec<u16>, u16)> = self
                .ligatures
                .iter()
                .enumerate()
                .map(|(at, chars)| {
                    let glyphs = chars.iter().map(|&ch| self.g(ch)).collect();
                    (glyphs, short(self.ligature_glyph(at)))
                })
                .collect();
            feature(*b"liga", lookup(4, ligature_subst(&ligatures)));
        }
        let localized = self.localized.map(|(system, ch)| {
            let pair = (self.g(ch), short(self.localized_glyph()));
            (system, feature(*b"locl", lookup(1, single_subst(&[pair]))))
        });
        for (tag, chars) in &self.alternates {
            let mut pairs: Vec<(u32, u32)> = chars
                .iter()
                .map(|&(ch, _)| (self.glyph(ch), self.alternate_glyph(*tag, ch)))
                .collect();
            pairs.sort();
            let subtable = if pairs.iter().all(|&(_, to)| to <= u32::from(u16::MAX)) {
                let pairs: Vec<(u16, u16)> = pairs
                    .iter()
                    .map(|&(from, to)| (short(from), short(to)))
                    .collect();
                single_subst(&pairs)
            } else {
                wide_single_subst(&pairs)
            };
            feature(*tag, lookup(1, subtable));
        }
        // The substitution is a lookup of its own, which only the contextual
        // lookup the feature names reaches.
        for (tag, chars) in &self.contextual {
            let mut pairs: Vec<(u16, u16)> = chars
                .iter()
                .map(|&(ch, _)| (self.g(ch), short(self.contextual_glyph(*tag, ch))))
                .collect();
            pairs.sort();
            let single = lookups.len() as u16;
            lookups.push(lookup(1, single_subst(&pairs)));
            let glyphs = pairs.iter().map(|pair| pair.0);
            features.push((*tag, vec![lookups.len() as u16]));
            lookups.push(lookup(5, context_subst(coverage(glyphs), single)));
        }
        (!features.is_empty()).then(|| layout_table(&self.scripts, &features, lookups, localized))
    }

    /// `GPOS`, or `None` where nothing is positioned.
    fn gpos(&self) -> Option<Vec<u8>> {
        let mut lookups = Vec::new();
        if !self.kerning.is_empty() {
            let mut pairs: Vec<(u16, u16, i16)> = self
                .kerning
                .iter()
                .map(|&(a, b, units)| (self.g(a), self.g(b), units))
                .collect();
            pairs.sort();
            lookups.push(lookup(2, pair_pos(&pairs)));
        }
        if !self.raised.is_empty() {
            let mut values: Vec<(u16, i16)> = self
                .raised
                .iter()
                .map(|&(ch, units)| (self.g(ch), units))
                .collect();
            values.sort();
            lookups.push(lookup(1, single_pos(&values)));
        }
        let mut features = Vec::new();
        if !lookups.is_empty() {
            features.push((*b"kern", (0..lookups.len() as u16).collect()));
        }
        if !self.halt.is_empty() {
            let mut values: Vec<(u16, i16, i16)> = self
                .halt
                .iter()
                .map(|&(ch, placement, advance)| (self.g(ch), placement, advance))
                .collect();
            values.sort();
            features.push((*b"halt", vec![lookups.len() as u16]));
            lookups.push(lookup(1, single_pos_x(&values)));
        }
        if !self.vhal.is_empty() {
            let mut values: Vec<(u16, i16, i16)> = self
                .vhal
                .iter()
                .map(|&(ch, placement, advance)| (self.g(ch), placement, advance))
                .collect();
            values.sort();
            features.push((*b"vhal", vec![lookups.len() as u16]));
            lookups.push(lookup(1, single_pos_y(&values)));
        }
        if !self.chws.is_empty() {
            let mut pairs: Vec<(u16, u16, i16)> = self
                .chws
                .iter()
                .map(|&(a, b, units)| (self.g(a), self.g(b), units))
                .collect();
            pairs.sort();
            features.push((*b"chws", vec![lookups.len() as u16]));
            lookups.push(lookup(2, pair_pos(&pairs)));
        }
        if lookups.is_empty() {
            return None;
        }
        Some(layout_table(&self.scripts, &features, lookups, None))
    }
}

fn push16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push24(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes()[1..]);
}

/// Returns `glyph` as a 16-bit table names it.
///
/// Panics where it is past 65,535: only alternates are written in 24 bits.
fn short(glyph: u32) -> u16 {
    u16::try_from(glyph).expect("a glyph a 16-bit table can name")
}

/// Every glyph's advance as a long metric, with no side bearing.
fn hmtx(advances: &[u16]) -> Vec<u8> {
    let mut table = Vec::new();
    for &advance in advances {
        push16(&mut table, advance);
        push16(&mut table, 0);
    }
    table
}

/// `vhea`, version 1.1, with a long metric for every glyph.
fn vhea(glyphs: u16) -> Vec<u8> {
    let mut table = vec![0u8; 36];
    table[0..4].copy_from_slice(&0x0001_1000u32.to_be_bytes());
    table[34..36].copy_from_slice(&glyphs.to_be_bytes());
    table
}

/// `VORG`: the default origin, and each listed glyph's, ascending.
fn vorg(default: i16, listed: &[(u16, i16)]) -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 1);
    push16(&mut table, 0);
    push16(&mut table, default as u16);
    push16(&mut table, listed.len() as u16);
    for &(glyph, y) in listed {
        push16(&mut table, glyph);
        push16(&mut table, y as u16);
    }
    table
}

fn maxp(glyphs: u16) -> Vec<u8> {
    let mut table = Vec::new();
    push32(&mut table, 0x0000_5000);
    push16(&mut table, glyphs);
    table
}

/// A `name` table giving the family, in English, for Windows.
fn name(family: &str) -> Vec<u8> {
    let encoded: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut table = Vec::new();
    push16(&mut table, 0); // format
    push16(&mut table, 1); // one record
    push16(&mut table, 6 + 12); // storage
    push16(&mut table, 3); // Windows
    push16(&mut table, 1); // Unicode BMP
    push16(&mut table, 0x0409); // English
    push16(&mut table, 1); // family name
    push16(&mut table, encoded.len() as u16);
    push16(&mut table, 0);
    table.extend_from_slice(&encoded);
    table
}

/// A `post` table, version 3, with an underline `position` units up and
/// `thickness` thick.
fn post(position: i16, thickness: i16) -> Vec<u8> {
    let mut table = vec![0u8; 32];
    table[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    table[8..10].copy_from_slice(&position.to_be_bytes());
    table[10..12].copy_from_slice(&thickness.to_be_bytes());
    table
}

/// `fvar`, version 1.0, with `axes` and no named instances.
fn fvar(axes: &[TestAxis]) -> Vec<u8> {
    let fixed = |value: f32| ((value * 65536.0) as i32).to_be_bytes();
    let mut table = Vec::new();
    push16(&mut table, 1);
    push16(&mut table, 0);
    push16(&mut table, 16); // axes array offset
    push16(&mut table, 2); // reserved
    push16(&mut table, axes.len() as u16);
    push16(&mut table, 20); // axis size
    push16(&mut table, 0); // instances
    push16(&mut table, 4 + 4 * axes.len() as u16); // instance size
    for axis in axes {
        table.extend_from_slice(&axis.tag);
        table.extend_from_slice(&fixed(axis.min));
        table.extend_from_slice(&fixed(axis.default));
        table.extend_from_slice(&fixed(axis.max));
        push16(&mut table, 0); // flags
        push16(&mut table, 256); // name
    }
    table
}

/// `HVAR`, version 1.0, with no mapping, so glyph `g` is item `g` of the one
/// item variation data: a region per axis, from its default to its maximum
/// peaking there, and every glyph moved by the axis's delta in it.
fn hvar(axes: &[TestAxis], glyphs: u16) -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 1);
    push16(&mut table, 0);
    push32(&mut table, 20); // item variation store
    push32(&mut table, 0); // advance mapping
    push32(&mut table, 0); // left side bearing mapping
    push32(&mut table, 0); // right side bearing mapping
    // The item variation store: format, region list offset, one data.
    let regions = axes.len() as u16;
    push16(&mut table, 1);
    push32(&mut table, 12);
    push16(&mut table, 1);
    let region_list_len = 4 + 6 * u32::from(regions) * u32::from(regions);
    push32(&mut table, 12 + region_list_len);
    push16(&mut table, regions); // axes
    push16(&mut table, regions); // regions
    for region in 0..regions {
        for axis in 0..regions {
            let peak: i16 = if axis == region { 0x4000 } else { 0 };
            push16(&mut table, 0); // start
            table.extend_from_slice(&peak.to_be_bytes());
            table.extend_from_slice(&peak.to_be_bytes()); // end
        }
    }
    push16(&mut table, glyphs); // items
    push16(&mut table, regions); // every delta a word
    push16(&mut table, regions);
    for region in 0..regions {
        push16(&mut table, region);
    }
    for _ in 0..glyphs {
        for axis in axes {
            table.extend_from_slice(&axis.delta.to_be_bytes());
        }
    }
    table
}

/// An empty `COLR` table, version 0: enough to make a font a color font.
fn colr() -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 0);
    push16(&mut table, 0);
    push32(&mut table, 14);
    push32(&mut table, 14);
    push16(&mut table, 0);
    table
}

/// A layout table's header, version 1.0, whose lists all lie past its end.
fn broken_layout() -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 1);
    push16(&mut table, 0);
    for _ in 0..3 {
        push16(&mut table, 0xFFF0);
    }
    table
}

/// `head` followed by `children`, the offset of each from the start of
/// `head` written into it at the matching position of `slots`.
///
/// How every table below is laid out: a header of counts and 16-bit offsets,
/// then what the offsets point at.
fn parent(mut head: Vec<u8>, slots: &[usize], children: Vec<Vec<u8>>) -> Vec<u8> {
    let mut offset = head.len();
    for (&slot, child) in slots.iter().zip(&children) {
        let at = u16::try_from(offset).expect("a test layout table under 64 KiB");
        head[slot..slot + 2].copy_from_slice(&at.to_be_bytes());
        offset += child.len();
    }
    for child in children {
        head.extend_from_slice(&child);
    }
    head
}

/// A coverage table, format 1, of `glyphs` in ascending order.
fn coverage(glyphs: impl ExactSizeIterator<Item = u16>) -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 1);
    push16(&mut table, glyphs.len() as u16);
    for glyph in glyphs {
        push16(&mut table, glyph);
    }
    table
}

/// A lookup of `kind` with the one `subtable`.
fn lookup(kind: u16, subtable: Vec<u8>) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, kind);
    push16(&mut head, 0); // flags
    push16(&mut head, 1);
    push16(&mut head, 0);
    parent(head, &[6], vec![subtable])
}

/// A single substitution, format 4, of `pairs` sorted by their first glyph:
/// 24-bit substitutes, over a 24-bit coverage, format 3.
fn wide_single_subst(pairs: &[(u32, u32)]) -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 4);
    push32(&mut table, (9 + 3 * pairs.len()) as u32); // coverage
    push24(&mut table, pairs.len() as u32);
    for &(_, to) in pairs {
        push24(&mut table, to);
    }
    push16(&mut table, 3);
    push24(&mut table, pairs.len() as u32);
    for &(from, _) in pairs {
        push24(&mut table, from);
    }
    table
}

/// A single substitution, format 2, of `pairs` sorted by their first glyph.
fn single_subst(pairs: &[(u16, u16)]) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 2);
    push16(&mut head, 0); // coverage
    push16(&mut head, pairs.len() as u16);
    for &(_, to) in pairs {
        push16(&mut head, to);
    }
    parent(head, &[2], vec![coverage(pairs.iter().map(|p| p.0))])
}

/// A contextual substitution, format 3, whose context is one glyph of
/// `coverage`, applying lookup `apply` to it.
fn context_subst(coverage: Vec<u8>, apply: u16) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 3);
    push16(&mut head, 1); // glyphs in the context
    push16(&mut head, 1); // lookups applied
    push16(&mut head, 0); // coverage
    push16(&mut head, 0); // at the context's first glyph
    push16(&mut head, apply);
    parent(head, &[6], vec![coverage])
}

/// A multiple substitution, format 1, of `sequences` sorted by their glyph.
fn multiple_subst(sequences: &[(u16, Vec<u16>)]) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 1);
    push16(&mut head, 0); // coverage
    push16(&mut head, sequences.len() as u16);
    let mut slots = Vec::new();
    let mut children = Vec::new();
    for (_, glyphs) in sequences {
        slots.push(head.len());
        push16(&mut head, 0);
        let mut sequence = Vec::new();
        push16(&mut sequence, glyphs.len() as u16);
        for &glyph in glyphs {
            push16(&mut sequence, glyph);
        }
        children.push(sequence);
    }
    slots.push(2);
    children.push(coverage(sequences.iter().map(|s| s.0)));
    parent(head, &slots, children)
}

/// A ligature substitution, format 1: each sequence of glyphs becomes its
/// ligature glyph.
fn ligature_subst(ligatures: &[(Vec<u16>, u16)]) -> Vec<u8> {
    let mut firsts: Vec<u16> = ligatures.iter().map(|(glyphs, _)| glyphs[0]).collect();
    firsts.sort();
    firsts.dedup();
    let mut head = Vec::new();
    push16(&mut head, 1);
    push16(&mut head, 0); // coverage
    push16(&mut head, firsts.len() as u16);
    let mut slots = Vec::new();
    let mut sets = Vec::new();
    for &first in &firsts {
        slots.push(head.len());
        push16(&mut head, 0);
        // Longest first, as a ligature set is tried in order.
        let mut these: Vec<&(Vec<u16>, u16)> = ligatures
            .iter()
            .filter(|(glyphs, _)| glyphs[0] == first)
            .collect();
        these.sort_by_key(|(glyphs, _)| Reverse(glyphs.len()));
        let mut set_head = Vec::new();
        push16(&mut set_head, these.len() as u16);
        let mut set_slots = Vec::new();
        let mut entries = Vec::new();
        for (glyphs, ligature) in these {
            set_slots.push(set_head.len());
            push16(&mut set_head, 0);
            let mut entry = Vec::new();
            push16(&mut entry, *ligature);
            push16(&mut entry, glyphs.len() as u16);
            for &glyph in &glyphs[1..] {
                push16(&mut entry, glyph);
            }
            entries.push(entry);
        }
        sets.push(parent(set_head, &set_slots, entries));
    }
    slots.push(2);
    sets.push(coverage(firsts.iter().copied()));
    parent(head, &slots, sets)
}

/// A single positioning, format 2, moving each glyph of `values`, sorted by
/// glyph, up by its units.
fn single_pos(values: &[(u16, i16)]) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 2);
    push16(&mut head, 0); // coverage
    push16(&mut head, 0x0002); // YPlacement
    push16(&mut head, values.len() as u16);
    for &(_, units) in values {
        head.extend_from_slice(&units.to_be_bytes());
    }
    parent(head, &[2], vec![coverage(values.iter().map(|v| v.0))])
}

/// A single positioning, format 2, moving each glyph along the line and
/// changing its advance, of `values` sorted by glyph.
fn single_pos_x(values: &[(u16, i16, i16)]) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 2);
    push16(&mut head, 0); // coverage
    push16(&mut head, 0x0005); // XPlacement, XAdvance
    push16(&mut head, values.len() as u16);
    for &(_, placement, advance) in values {
        head.extend_from_slice(&placement.to_be_bytes());
        head.extend_from_slice(&advance.to_be_bytes());
    }
    parent(head, &[2], vec![coverage(values.iter().map(|v| v.0))])
}

/// A single positioning, format 2, moving each glyph up and changing its
/// vertical advance, of `values` sorted by glyph.
fn single_pos_y(values: &[(u16, i16, i16)]) -> Vec<u8> {
    let mut head = Vec::new();
    push16(&mut head, 2);
    push16(&mut head, 0); // coverage
    push16(&mut head, 0x000A); // YPlacement, YAdvance
    push16(&mut head, values.len() as u16);
    for &(_, placement, advance) in values {
        head.extend_from_slice(&placement.to_be_bytes());
        head.extend_from_slice(&advance.to_be_bytes());
    }
    parent(head, &[2], vec![coverage(values.iter().map(|v| v.0))])
}

/// A pair positioning, format 1, adding to each pair's first advance, of
/// `pairs` sorted by first glyph then second.
fn pair_pos(pairs: &[(u16, u16, i16)]) -> Vec<u8> {
    let mut firsts: Vec<u16> = pairs.iter().map(|p| p.0).collect();
    firsts.dedup();
    let mut head = Vec::new();
    push16(&mut head, 1);
    push16(&mut head, 0); // coverage
    push16(&mut head, 0x0004); // XAdvance on the first
    push16(&mut head, 0); // nothing on the second
    push16(&mut head, firsts.len() as u16);
    let mut slots = Vec::new();
    let mut sets = Vec::new();
    for &first in &firsts {
        slots.push(head.len());
        push16(&mut head, 0);
        let these: Vec<_> = pairs.iter().filter(|p| p.0 == first).collect();
        let mut set = Vec::new();
        push16(&mut set, these.len() as u16);
        for &&(_, second, units) in &these {
            push16(&mut set, second);
            set.extend_from_slice(&units.to_be_bytes());
        }
        sets.push(set);
    }
    slots.push(2);
    sets.push(coverage(firsts.iter().copied()));
    parent(head, &slots, sets)
}

/// `trak` with one horizontal track, the normal one, tracking by `sizes`:
/// each point size and the units it adds there.
fn trak(sizes: &[(f32, i16)]) -> Vec<u8> {
    let count = sizes.len() as u16;
    // The header, then the horizontal data: its counts and its size
    // table's offset, its one entry, the size table, and the values, every
    // offset from the table's start.
    let data = 12u16;
    let size_table = u32::from(data) + 8 + 8;
    let values = size_table + 4 * u32::from(count);
    let mut table = Vec::new();
    push32(&mut table, 0x0001_0000); // version
    push16(&mut table, 0); // format
    push16(&mut table, data); // horizontal
    push16(&mut table, 0); // no vertical
    push16(&mut table, 0); // reserved
    push16(&mut table, 1); // one track
    push16(&mut table, count);
    push32(&mut table, size_table);
    push32(&mut table, 0); // the normal track, 0.0
    push16(&mut table, 256); // its name
    push16(&mut table, values as u16);
    for &(size, _) in sizes {
        push32(&mut table, (size * 65536.0) as u32);
    }
    for &(_, units) in sizes {
        table.extend_from_slice(&units.to_be_bytes());
    }
    table
}

/// A `STAT` table naming nothing, version 1.1: HarfBuzz applies `trak` only
/// to a font with one, as the fonts of the systems that track have.
fn stat() -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 1); // major version
    push16(&mut table, 1); // minor version
    push16(&mut table, 8); // design axis size
    push16(&mut table, 0); // no design axes
    push32(&mut table, 0);
    push16(&mut table, 0); // no axis values
    push32(&mut table, 0);
    push16(&mut table, 2); // elided fallback name
    table
}

/// A language system turning on the features at `indices`.
fn lang_sys(indices: &[u16]) -> Vec<u8> {
    let mut table = Vec::new();
    push16(&mut table, 0); // lookup order
    push16(&mut table, 0xFFFF); // no required feature
    push16(&mut table, indices.len() as u16);
    for &index in indices {
        push16(&mut table, index);
    }
    table
}

/// A `GSUB` or `GPOS` table, version 1.0, with `features` (each a tag and
/// its lookups, in tag order) and `lookups`, under the script tags
/// `scripts`, ascending.
///
/// Every feature is in each script's default language system but the one
/// `localized` names, which is only in its own language system of `latn`,
/// with the rest.
fn layout_table(
    scripts: &[[u8; 4]],
    features: &[([u8; 4], Vec<u16>)],
    lookups: Vec<Vec<u8>>,
    localized: Option<([u8; 4], u16)>,
) -> Vec<u8> {
    // The features' records in tag order, their indices following.
    let mut order: Vec<usize> = (0..features.len()).collect();
    order.sort_by_key(|&at| features[at].0);
    let index_of = |at: usize| order.iter().position(|&o| o == at).unwrap() as u16;
    let defaults: Vec<u16> = (0..features.len())
        .filter(|&at| localized.is_none_or(|(_, local)| usize::from(local) != at))
        .map(index_of)
        .collect();
    let script = |with_local: bool| {
        let mut head = Vec::new();
        push16(&mut head, 0); // default language system
        let mut slots = vec![0];
        let mut children = vec![lang_sys(&defaults)];
        match localized {
            Some((system, local)) if with_local => {
                push16(&mut head, 1);
                head.extend_from_slice(&system);
                slots.push(head.len());
                push16(&mut head, 0);
                let mut all = defaults.clone();
                all.push(index_of(usize::from(local)));
                children.push(lang_sys(&all));
            }
            _ => push16(&mut head, 0),
        }
        parent(head, &slots, children)
    };
    let mut list = Vec::new();
    push16(&mut list, scripts.len() as u16);
    let mut slots = Vec::new();
    let mut children = Vec::new();
    for tag in scripts {
        list.extend_from_slice(tag);
        slots.push(list.len());
        push16(&mut list, 0);
        children.push(script(tag == b"latn"));
    }
    let scripts = parent(list, &slots, children);

    let mut feature_list = Vec::new();
    push16(&mut feature_list, features.len() as u16);
    let mut slots = Vec::new();
    let mut tables = Vec::new();
    for &at in &order {
        let (tag, indices) = &features[at];
        feature_list.extend_from_slice(tag);
        slots.push(feature_list.len());
        push16(&mut feature_list, 0);
        let mut table = Vec::new();
        push16(&mut table, 0); // no parameters
        push16(&mut table, indices.len() as u16);
        for &index in indices {
            push16(&mut table, index);
        }
        tables.push(table);
    }
    let feature_list = parent(feature_list, &slots, tables);

    let mut lookup_list = Vec::new();
    push16(&mut lookup_list, lookups.len() as u16);
    let slots: Vec<usize> = (0..lookups.len()).map(|at| 2 + 2 * at).collect();
    for _ in 0..lookups.len() {
        push16(&mut lookup_list, 0);
    }
    let lookup_list = parent(lookup_list, &slots, lookups);

    let mut head = Vec::new();
    push16(&mut head, 1);
    push16(&mut head, 0);
    for _ in 0..3 {
        push16(&mut head, 0);
    }
    parent(head, &[4, 6, 8], vec![scripts, feature_list, lookup_list])
}

/// A font holding `tables`, their directory sorted by tag as readers search
/// it, each table on a four-byte boundary.
fn sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|(tag, _)| *tag);
    let mut font = Vec::new();
    push32(&mut font, 0x0001_0000);
    push16(&mut font, tables.len() as u16);
    for _ in 0..3 {
        push16(&mut font, 0); // search fields, unread
    }
    let mut offset = 12 + 16 * tables.len();
    for (tag, table) in &tables {
        font.extend_from_slice(tag);
        push32(&mut font, 0); // checksum, unread
        push32(&mut font, offset as u32);
        push32(&mut font, table.len() as u32);
        offset += table.len().next_multiple_of(4);
    }
    for (_, table) in &tables {
        font.extend_from_slice(table);
        font.resize(font.len().next_multiple_of(4), 0);
    }
    font
}
