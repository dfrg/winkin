//! UAX #14's rules: the line classes, the states a scan is in after a
//! character, and the decision between two characters.
//!
//! [`decide`] is the one place the rules are written, in UAX #14's order,
//! with the options' tailorings after the rules no tailoring may change
//! (LB4 to LB12). It reads the state before a boundary, the class after it,
//! and at most two classes of lookahead, which it asks for by returning
//! [`NEEDS_NEXT`] or [`NEEDS_SECOND`]. [`next_left`] moves the state past a
//! character. [`PAIRS`] holds [`decide`]'s answer for every pair of state and
//! class it decides from those two alone, built from it at compile time: the
//! scan's common path is one load from it.
//!
//! Every regular context a rule reads (`SP*`, the number before a postfix,
//! an odd run of regional indicators) is a state of its own: SP after OP is
//! [`SP_OP`], not SP with a flag. East_Asian_Width and the zero width joiner
//! are flags beside the state, since they say something about a character
//! rather than the text before it.

// The line classes: Line_Break as the line byte holds it (the generator's
// `line_class`), split where a rule or an option reads more.

/// XX, unassigned and private use: AL by LB1, apart only for `keep-all`.
pub(super) const XX: u8 = 0;
pub(super) const AL: u8 = 1;
/// AI: AL by LB1, apart only for `break-all`.
pub(super) const AI: u8 = 2;
/// U+25CC DOTTED CIRCLE, an AL that LB28a names.
pub(super) const DOTTED: u8 = 3;
/// SA that is neither Mn nor Mc: AL by LB1.
pub(super) const SA: u8 = 4;
/// SA that is Mn or Mc: CM by LB1.
pub(super) const SA_MARK: u8 = 5;
pub(super) const BK: u8 = 6;
pub(super) const CR: u8 = 7;
pub(super) const LF: u8 = 8;
pub(super) const NL: u8 = 9;
pub(super) const SP: u8 = 10;
pub(super) const ZW: u8 = 11;
pub(super) const WJ: u8 = 12;
pub(super) const GL: u8 = 13;
pub(super) const CM: u8 = 14;
pub(super) const ZWJ: u8 = 15;
pub(super) const CL: u8 = 16;
pub(super) const CP: u8 = 17;
pub(super) const EX: u8 = 18;
pub(super) const IS: u8 = 19;
pub(super) const SY: u8 = 20;
/// OP that is not East Asian, which LB30 holds.
pub(super) const OP: u8 = 21;
/// OP that is East Asian.
pub(super) const OP_EA: u8 = 22;
/// QU that is neither Pi nor Pf.
pub(super) const QU: u8 = 23;
/// QU that is Pi, an initial quotation mark.
pub(super) const QU_PI: u8 = 24;
/// QU that is Pf, a final quotation mark.
pub(super) const QU_PF: u8 = 25;
pub(super) const NS: u8 = 26;
/// CJ: NS under `strict`, ID otherwise.
pub(super) const CJ: u8 = 27;
pub(super) const B2: u8 = 28;
pub(super) const BA: u8 = 29;
pub(super) const BB: u8 = 30;
pub(super) const HY: u8 = 31;
pub(super) const HH: u8 = 32;
pub(super) const CB: u8 = 33;
pub(super) const IN: u8 = 34;
pub(super) const NU: u8 = 35;
pub(super) const PO: u8 = 36;
pub(super) const PR: u8 = 37;
pub(super) const HL: u8 = 38;
pub(super) const ID: u8 = 39;
/// ID that is Extended_Pictographic and unassigned, which LB30b reads.
pub(super) const ID_CN: u8 = 40;
pub(super) const EB: u8 = 41;
pub(super) const EM: u8 = 42;
pub(super) const JL: u8 = 43;
pub(super) const JV: u8 = 44;
pub(super) const JT: u8 = 45;
pub(super) const H2: u8 = 46;
pub(super) const H3: u8 = 47;
pub(super) const RI: u8 = 48;
pub(super) const AK: u8 = 49;
pub(super) const AP: u8 = 50;
pub(super) const AS: u8 = 51;
pub(super) const VF: u8 = 52;
pub(super) const VI: u8 = 53;
/// PO that is East_Asian_Width A, F or W, which the Japanese and Chinese
/// `loose` tailoring breaks before.
pub(super) const PO_EAW: u8 = 54;
/// PR that is East_Asian_Width A, F or W, which the Japanese and Chinese
/// `loose` tailoring breaks after.
pub(super) const PR_EAW: u8 = 55;
/// NS that Japanese and Chinese `normal` and `loose` let a line break
/// before: U+301C `〜` and U+30A0 `゠`.
pub(super) const NS_DASH: u8 = 56;
/// NS that `loose` lets a line break before: the iteration marks U+3005,
/// U+303B, U+309D, U+309E, U+30FD and U+30FE.
pub(super) const NS_ITERATION: u8 = 57;
/// NS that Japanese and Chinese `loose` lets a line break before: the
/// centred punctuation U+30FB, U+FF1A, U+FF1B, U+FF65, U+203C and U+2047 to
/// U+2049.
pub(super) const NS_CENTRED: u8 = 58;
/// EX that Japanese and Chinese `loose` lets a line break before: U+FF01
/// `！` and U+FF1F `？`.
pub(super) const EX_WIDE: u8 = 59;

/// The number of classes the line byte holds.
pub(super) const DATA_CLASS_COUNT: usize = 60;

/// The Line_Break value of each class the line byte holds, in ICU4C's
/// `ULineBreak` numbering.
pub(super) const LINE_BREAK: [u8; DATA_CLASS_COUNT] = {
    let mut values = [0; DATA_CLASS_COUNT];
    let pairs: [(u8, u8); DATA_CLASS_COUNT] = [
        (XX, 0),
        (AL, 2),
        (AI, 1),
        (DOTTED, 2),
        (SA, 24),
        (SA_MARK, 24),
        (BK, 6),
        (CR, 10),
        (LF, 17),
        (NL, 29),
        (SP, 26),
        (ZW, 28),
        (WJ, 30),
        (GL, 12),
        (CM, 9),
        (ZWJ, 42),
        (CL, 8),
        (CP, 36),
        (EX, 11),
        (IS, 16),
        (SY, 27),
        (OP, 20),
        (OP_EA, 20),
        (QU, 23),
        (QU_PI, 23),
        (QU_PF, 23),
        (NS, 18),
        (CJ, 37),
        (B2, 3),
        (BA, 4),
        (BB, 5),
        (HY, 13),
        (HH, 48),
        (CB, 7),
        (IN, 15),
        (NU, 19),
        (PO, 21),
        (PR, 22),
        (HL, 38),
        (ID, 14),
        (ID_CN, 14),
        (EB, 40),
        (EM, 41),
        (JL, 33),
        (JV, 35),
        (JT, 34),
        (H2, 31),
        (H3, 32),
        (RI, 39),
        (AK, 43),
        (AP, 44),
        (AS, 45),
        (VF, 46),
        (VI, 47),
        (PO_EAW, 21),
        (PR_EAW, 22),
        (NS_DASH, 18),
        (NS_ITERATION, 18),
        (NS_CENTRED, 18),
        (EX_WIDE, 11),
    ];
    let mut i = 0;
    while i < DATA_CLASS_COUNT {
        let (class, value) = pairs[i];
        values[class as usize] = value;
        i += 1;
    }
    values
};

/// An NS or EX character that the strictness lets a line break before, as
/// ICU4C's rules read their `$NSX` and `$EXX`: no rule names it after a
/// boundary, so only the rules that hold before any character hold before
/// it. A class the options resolve to, never one of the line byte.
pub(super) const NSX: u8 = 60;

/// AL that `break-all` reads as ID: an ideograph to every rule but LB20a,
/// which still holds it after a hyphen starting a word: `‐a` stays whole
/// under `loose` and `break-all`. A class the options resolve to.
pub(super) const ID_LETTER: u8 = 61;

/// The number of classes the options resolve to.
pub(super) const CLASS_COUNT: usize = 62;

// The states that are a class read in a context: what precedes the
// character decides a later boundary.

/// The start of the text: no boundary here (LB2).
pub(super) const SOT: u8 = 62;
/// SP after ZW and any SP (LB8).
pub(super) const SP_ZW: u8 = 63;
/// SP after OP and any SP (LB14).
pub(super) const SP_OP: u8 = 64;
/// SP after a [`QU_PI_START`] and any SP (LB15a).
pub(super) const SP_QU: u8 = 65;
/// SP after CL or CP and any SP (LB16).
pub(super) const SP_CLCP: u8 = 66;
/// SP after B2 and any SP (LB17).
pub(super) const SP_B2: u8 = 67;
/// An initial quotation mark where LB15a holds what follows it: at the
/// start, or after BK, CR, LF, NL, OP, QU, GL, SP or ZW.
pub(super) const QU_PI_START: u8 = 68;
/// HY starting a word (LB20a): at the start, or after BK, CR, LF, NL, SP,
/// ZW, CB or GL.
pub(super) const HY_WORD: u8 = 69;
/// HH starting a word (LB20a).
pub(super) const HH_WORD: u8 = 70;
/// HY after HL (LB21a).
pub(super) const HY_HL: u8 = 71;
/// HH after HL (LB21a).
pub(super) const HH_HL: u8 = 72;
/// SY in a number: `NU (SY | IS)*` (LB25).
pub(super) const SY_NU: u8 = 73;
/// IS in a number: `NU (SY | IS)*` (LB25).
pub(super) const IS_NU: u8 = 74;
/// CL closing a number: `NU (SY | IS)* CL` (LB25).
pub(super) const CL_NU: u8 = 75;
/// CP closing a number: `NU (SY | IS)* CP` (LB25).
pub(super) const CP_NU: u8 = 76;
/// VI after AK, U+25CC or AS (LB28a).
pub(super) const VI_AK: u8 = 77;
/// RI ending an even run of them (LB30a); [`RI`] ends an odd run.
pub(super) const RI_EVEN: u8 = 78;
/// BA after HL, which Unicode 15.1's LB21a holds.
#[cfg(test)]
pub(super) const BA_HL: u8 = 79;
/// A CM or ZWJ read as AL (LB10) where ICU4X's segmenter keeps it apart
/// from AL for `keep-all`.
#[cfg(test)]
pub(super) const MARK_AL: u8 = 80;
/// U+25CC after U+25CC, where ICU4X's segmenter breaks before anything but
/// a letter, a non-East Asian OP, VF or an initial quotation mark.
#[cfg(test)]
pub(super) const DOTTED_PAIR: u8 = 81;

/// A set of classes and states, a bit each.
type Set = u128;

// Every class fits the line byte's six bits, and every state and lookahead
// value fits a `Set`.
const _: () = assert!(
    DATA_CLASS_COUNT <= 64 && CLASS_COUNT <= 64 && RI_EVEN < EOT && EOT < UNREAD && UNREAD < 128
);
#[cfg(test)]
const _: () = assert!(DOTTED_PAIR < EOT);

/// Returns the set of `members`.
const fn set(members: &[u8]) -> Set {
    let mut bits = 0;
    let mut at = 0;
    while at < members.len() {
        bits |= 1 << members[at];
        at += 1;
    }
    bits
}

/// Returns whether `class` is in `set`.
#[inline(always)]
const fn is(class: u8, set: Set) -> bool {
    // Every class and state is under 128.
    (set >> (class & 127)) & 1 != 0
}

/// The mandatory breaks (LB4, LB5, LB6).
const HARD: Set = set(&[BK, CR, LF, NL]);
/// SP in every context.
pub(super) const SPACES: Set = set(&[SP, SP_ZW, SP_OP, SP_QU, SP_CLCP, SP_B2]);
/// The states after which a CM or ZWJ is not part of the character before
/// it (LB9), and so is read as AL (LB10).
const UNMARKABLE: Set = set(&[
    SOT, BK, CR, LF, NL, ZW, SP, SP_ZW, SP_OP, SP_QU, SP_CLCP, SP_B2,
]);
/// Returns whether a CM or ZWJ after `state` stands alone rather than
/// joining the character before it: after the start, a mandatory break, ZW
/// or SP (LB9), where it reads as AL (LB10).
#[inline(always)]
pub(super) const fn is_unmarkable(state: u8) -> bool {
    is(state, UNMARKABLE)
}
/// QU in every context.
const QUOTES: Set = set(&[QU, QU_PI, QU_PF, QU_PI_START]);
/// OP, East Asian or not.
const OPENS: Set = set(&[OP, OP_EA]);
const HYPHENS: Set = set(&[HY, HY_WORD, HY_HL]);
const HARD_HYPHENS: Set = set(&[HH, HH_WORD, HH_HL]);
/// AL once LB1 has resolved AI, XX and SA; U+25CC is AL.
const ALPHABETIC: Set = set(&[AL, AI, XX, SA, DOTTED]) | MARKS_AS_AL;
/// The test state [`MARK_AL`], which reads as AL.
#[cfg(test)]
const MARKS_AS_AL: Set = set(&[MARK_AL]);
#[cfg(not(test))]
const MARKS_AS_AL: Set = 0;
/// `(AL | HL)`.
const LETTERS: Set = ALPHABETIC | set(&[HL]);
/// PO, with the East Asian ones Japanese and Chinese `loose` reads apart.
const POSTFIXES: Set = set(&[PO, PO_EAW]);
/// PR, with the East Asian ones Japanese and Chinese `loose` reads apart.
const PREFIXES: Set = set(&[PR, PR_EAW]);
/// `(PR | PO)`.
const AFFIXES: Set = POSTFIXES | PREFIXES;
/// `(PR | PO)` as LB24 reads them, and LB25 before a number: an East Asian
/// PR, which Japanese and Chinese `loose` breaks after, is left out, as
/// ICU4C leaves out its `$PRX`.
const AFFIXES_BESIDE_LETTERS: Set = set(&[PR, PO, PO_EAW]);
/// `(PR | PO)` as LB25 reads them after a number: an East Asian PO, which
/// Japanese and Chinese `loose` breaks before, is left out, as ICU4C leaves
/// out its `$POX`.
const AFFIXES_AFTER_NUMBERS: Set = set(&[PR, PR_EAW, PO]);
/// `(ID | EB | EM)`, LB23a's ideographs.
const IDEOGRAPHS: Set = set(&[ID, ID_CN, ID_LETTER, EB, EM]);
/// NS, with CJ under `strict`.
const NONSTARTERS: Set = set(&[NS, CJ]);
const KOREAN: Set = set(&[JL, JV, JT, H2, H3]);
/// `(AK | [◌] | AS)`, LB28a's bases.
const AKSARA: Set = set(&[AK, DOTTED, AS]);
/// The states that end `NU (SY | IS)*`.
const NUMBER: Set = set(&[NU, SY_NU, IS_NU]);
/// CL and CP, closing a number or not (LB16).
const CLOSES: Set = set(&[CL, CP, CL_NU, CP_NU]);
/// What LB15b holds a final quotation mark before: `SP | GL | WJ | CL | QU |
/// CP | EX | IS | SY | BK | CR | LF | NL | ZW`, or the end.
const QUOTE_FOLLOWERS: Set = set(&[
    SP, GL, WJ, CL, QU, QU_PI, QU_PF, CP, EX, IS, SY, BK, CR, LF, NL, ZW, EOT,
]);
/// What LB15a holds an initial quotation mark after: `sot | BK | CR | LF |
/// NL | OP | QU | GL | SP | ZW`.
const QUOTE_OPENERS: Set = set(&[SOT, BK, CR, LF, NL, OP, OP_EA, GL, ZW]) | QUOTES | SPACES;
/// What LB20a reads a hyphen after as starting a word: `sot | BK | CR | LF |
/// NL | SP | ZW | CB | GL`.
const WORD_STARTS: Set = set(&[SOT, BK, CR, LF, NL, ZW, CB, GL]) | SPACES;
/// What `keep-all` never breaks between: letters, digits, ideographs, Hangul
/// and CJ. A line still breaks after a hyphen-minus there (`漢-|a`), and CSS
/// names only NU, AL, AI and ID.
const KEEP_ALL: Set = set(&[AI, AL, ID, NU, H2, H3, JL, JV, JT, CJ]);
/// What ICU4X's `keep-all` never breaks between: [`KEEP_ALL`] and HY.
#[cfg(test)]
const ICU_KEEP_ALL: Set = KEEP_ALL | set(&[HY, HY_WORD, HY_HL]);
/// The ideographs ICU4C's Japanese and Chinese `loose` rules break after
/// before a hyphen (`$ID`, with CJ, which `loose` reads as ID).
const LOOSE_HYPHEN_BASES: Set = set(&[ID, ID_CN, ID_LETTER]);

/// The end of the text, as lookahead reads it.
pub(super) const EOT: u8 = 126;
/// Lookahead not read yet.
pub(super) const UNREAD: u8 = 127;

// What `decide` answers.

/// A line may not break here.
pub(super) const KEEP: u8 = 0;
/// A line may break here.
pub(super) const BREAK: u8 = 1;
/// The answer needs the class after the right character: call again with
/// `next`.
pub(super) const NEEDS_NEXT: u8 = 2;
/// The answer needs the class after that: call again with `second`.
pub(super) const NEEDS_SECOND: u8 = 3;
/// The answer needs the East Asian and pictograph flags, which the pair
/// table leaves unread.
const NEEDS_FLAGS: u8 = 4;

// The flags of a pair.

/// The character before the boundary is East Asian.
pub(super) const LEFT_EAST_ASIAN: u8 = 1;
/// The character before that is East Asian, or the boundary follows the
/// start; read where the left character is a quotation mark (LB19a).
pub(super) const BEFORE_EAST_ASIAN: u8 = 2;
/// The character after the boundary is East Asian.
pub(super) const RIGHT_EAST_ASIAN: u8 = 4;
/// The character just before the boundary is a zero width joiner (LB8a).
pub(super) const AFTER_ZWJ: u8 = 8;
/// The class after the right one is East Asian.
pub(super) const NEXT_EAST_ASIAN: u8 = 16;
/// ICU4X's segmenter is partway through a rule of more than two
/// characters, where it reads no tailoring.
#[cfg(test)]
pub(super) const ICU_INNER: u8 = 32;
/// As [`ICU_INNER`], where a final quotation mark after a space is followed
/// by what LB15b holds it before.
#[cfg(test)]
pub(super) const ICU_INNER_BEFORE_FOLLOWER: u8 = 64;
/// The character before the boundary is Extended_Pictographic and
/// unassigned (LB30b).
pub(super) const LEFT_UNASSIGNED_PICTOGRAPH: u8 = 128;

/// Returns the state flags of a character with the line byte flags `bits`:
/// [`LEFT_EAST_ASIAN`] and [`LEFT_UNASSIGNED_PICTOGRAPH`].
#[inline(always)]
pub(super) const fn left_flags(bits: u8) -> u8 {
    // The line byte's bits 6 and 7.
    ((bits >> 6) & 1) * LEFT_EAST_ASIAN + (bits >> 7) * LEFT_UNASSIGNED_PICTOGRAPH
}

/// How a strictness tailors the rules: none, the Japanese and Chinese
/// `normal` breaks, or `loose`.
///
/// The characters a strictness lets a line break before or after are
/// classes of their own once resolved ([`NSX`], [`PO_EAW`], [`PR_EAW`]), as
/// ICU4C's rules make them; the rest are [`tailor`]'s.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Tailoring {
    None,
    NormalJapaneseChinese,
    Loose,
}

/// The options [`decide`] reads.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Rules {
    pub(super) tailoring: Tailoring,
    /// Whether `loose` takes the Japanese and Chinese breaks too.
    pub(super) ja_zh: bool,
    pub(super) keep_all: bool,
    /// Whether a line breaks before U+2010 and U+2013 after an ideograph:
    /// under `loose`, in Japanese and Chinese or with `break-all`.
    pub(super) loose_hyphens: bool,
    /// The older rules and ICU4X quirks the differential tests switch on,
    /// as [`Legacy`] bits.
    #[cfg(test)]
    pub(super) legacy: u32,
}

/// The rules of older Unicode versions, and the quirks of ICU4X's segmenter,
/// that the differential tests put back one by one. Outside tests no rules
/// carry them, and every test of them is false.
pub(super) struct Legacy;

impl Legacy {
    /// Unicode 15.1's LB12a: `[^SP BA HY] × GL`.
    pub(super) const LB12A: u32 = 1;
    /// Unicode 15.1's LB13, which holds IS too, without LB15c and LB15d.
    pub(super) const LB13: u32 = 2;
    /// Unicode 15.1's LB19, `× QU` and `QU ×`, without LB19a.
    pub(super) const LB19: u32 = 4;
    /// No LB20a, which Unicode 16.0 adds.
    pub(super) const LB20A: u32 = 8;
    /// Unicode 15.1's LB21a: `HL (HY | BA) ×`.
    pub(super) const LB21A: u32 = 16;
    /// Unicode 15.1's LB25, pairs of classes.
    pub(super) const LB25: u32 = 32;
    /// Every rule of Unicode 15.1.
    #[cfg(test)]
    pub(super) const UNICODE_15_1: u32 =
        Self::LB12A | Self::LB13 | Self::LB19 | Self::LB20A | Self::LB21A | Self::LB25;
    /// ICU4X: `loose` without the Japanese and Chinese tailoring holds the
    /// pairs that tailoring breaks.
    #[cfg(test)]
    pub(super) const LOOSE_HOLDS: u32 = 64;
    /// ICU4X: the tailorings come before every rule but LB8a and LB9, so a
    /// `normal` or `loose` break may follow WJ or GL.
    pub(super) const TAILORING_FIRST: u32 = 128;
    /// ICU4X: SA reads as AL even where it is Mn or Mc, and as ID under
    /// `break-all`.
    #[cfg(test)]
    pub(super) const SA_MARKS: u32 = 256;
    /// ICU4X: a CM or ZWJ read as AL (LB10) is not AL for `keep-all`.
    #[cfg(test)]
    pub(super) const MARKS_APART: u32 = 512;
    /// ICU4X: no tailoring applies partway through a rule of more than two
    /// characters, as after `OP SP*` or `HL HY`.
    #[cfg(test)]
    pub(super) const TAILORING_OUTSIDE_RULES: u32 = 1024;
    /// ICU4X: after two U+25CC a line breaks before anything but a letter, a
    /// non-East Asian OP, VF or an initial quotation mark.
    pub(super) const DOTTED_PAIRS: u32 = 2048;
    /// ICU4X: after a break, or a pair a tailoring decides, the next
    /// character is read with no context, as at the start.
    #[cfg(test)]
    pub(super) const CONTEXT_FORGOTTEN: u32 = 4096;
    /// ICU4X: the strictness tailorings break before or after the characters
    /// they name, whatever the context after LB12, rather than reading them
    /// as classes of their own; `loose` breaks before a hyphen after an
    /// ideograph in every language. `LOOSE_HOLDS`, `TAILORING_FIRST` and
    /// `TAILORING_OUTSIDE_RULES` change these tailorings.
    pub(super) const ICU_TAILORING: u32 = 8192;
    /// ICU4X: `keep-all` holds HY too.
    pub(super) const KEEP_ALL_HYPHENS: u32 = 16384;
    /// ICU4X: a line breaks at the end of each script's part of a run of
    /// two or more SA characters, whatever follows, even a space.
    #[cfg(test)]
    pub(super) const RUN_END_BREAKS: u32 = 1 << 15;
    /// ICU4X: U+0EA3 LAO LETTER LO LING is outside its Lao, so a Lao run
    /// breaks before and after it.
    #[cfg(test)]
    pub(super) const LAO_LO_LING: u32 = 1 << 16;
    /// ICU4X: a word with no longer word after it ends where the dictionary
    /// says, even inside a grapheme cluster.
    #[cfg(test)]
    pub(super) const DICTIONARY_LEAVES: u32 = 1 << 17;
    /// ICU4X: where no word is found, everything read is one word, the
    /// character that missed with it, rather than one grapheme cluster.
    #[cfg(test)]
    pub(super) const DICTIONARY_MISSES: u32 = 1 << 18;
    /// ICU4X: at the end of a script's part of a run, after a word and the
    /// start of another that ends no word, the rest is one word.
    #[cfg(test)]
    pub(super) const DICTIONARY_REST: u32 = 1 << 19;
    /// A scan models ICU4X's SA runs instead of matching words itself, with
    /// none of ICU4X's ways but those the other switches put back.
    #[cfg(test)]
    pub(super) const RUN_MODEL: u32 = 1 << 20;
    /// Every switch of ICU4X's SA runs, under which a scan models them
    /// instead of matching words itself.
    #[cfg(test)]
    pub(super) const DICTIONARY_RUNS: u32 = Self::RUN_MODEL
        | Self::RUN_END_BREAKS
        | Self::LAO_LO_LING
        | Self::DICTIONARY_LEAVES
        | Self::DICTIONARY_MISSES
        | Self::DICTIONARY_REST;
}

/// Returns whether `rules` has the legacy bits `bits`; always false outside
/// tests.
#[inline(always)]
const fn legacy(rules: &Rules, bits: u32) -> bool {
    #[cfg(test)]
    {
        rules.legacy & bits != 0
    }
    #[cfg(not(test))]
    {
        let _ = (rules, bits);
        false
    }
}

/// Everything [`decide`] reads about one boundary.
#[derive(Copy, Clone, Debug)]
pub(super) struct Pair {
    /// The state before the boundary.
    pub(super) left: u8,
    /// The class of the character after it.
    pub(super) right: u8,
    /// The pair's flags: [`LEFT_EAST_ASIAN`] and the others.
    pub(super) flags: u8,
    /// Whether the East Asian and pictograph flags are unread, as where the
    /// pair table is built.
    pub(super) flags_unread: bool,
    /// The character after the boundary, which the tailorings read.
    pub(super) ch: char,
    /// The class after the right character's marks, [`EOT`] or [`UNREAD`].
    pub(super) next: u8,
    /// The class after that, [`EOT`] or [`UNREAD`].
    pub(super) second: u8,
}

/// Returns whether a line may break between `pair.left` and `pair.right`:
/// [`KEEP`], [`BREAK`], or the lookahead it needs.
///
/// CM and ZWJ are resolved before (LB9, LB10): a mark that joins the
/// character before it never reaches here, and one that does not is AL.
pub(super) const fn decide(pair: &Pair, rules: &Rules) -> u8 {
    let (l, r) = (pair.left, pair.right);
    if legacy(rules, Legacy::TAILORING_FIRST) {
        let first = icu_first(pair, rules);
        if first != UNREAD {
            return first;
        }
    }
    // LB4, LB5.
    if l == BK || l == LF || l == NL {
        return BREAK;
    }
    if l == CR {
        return if r == LF { KEEP } else { BREAK };
    }
    // LB6, LB7.
    if is(r, HARD) || r == SP || r == ZW {
        return KEEP;
    }
    // LB8.
    if l == ZW || l == SP_ZW {
        return BREAK;
    }
    // LB8a.
    if pair.flags & AFTER_ZWJ != 0 {
        return KEEP;
    }
    // LB11, LB12.
    if l == WJ || r == WJ || l == GL {
        return KEEP;
    }
    if !legacy(rules, Legacy::TAILORING_FIRST) && !icu_inner(pair, rules) {
        let tailored = tailor(pair, rules);
        if tailored != UNREAD {
            return tailored;
        }
    }
    // LB12a.
    if r == GL {
        let breaks_before = if legacy(rules, Legacy::LB12A) {
            SPACES | HYPHENS | set(&[BA])
        } else {
            SPACES | HYPHENS | HARD_HYPHENS
        };
        if !is(l, breaks_before) {
            return KEEP;
        }
    }
    // LB13.
    if r == CL || r == CP || r == EX || r == SY {
        return KEEP;
    }
    if legacy(rules, Legacy::LB13) && r == IS {
        return KEEP;
    }
    // LB14.
    if is(l, OPENS) || l == SP_OP {
        return KEEP;
    }
    // LB15a.
    if l == QU_PI_START || l == SP_QU {
        return KEEP;
    }
    // LB15b.
    if r == QU_PF {
        if pair.next == UNREAD {
            return NEEDS_NEXT;
        }
        if is(pair.next, QUOTE_FOLLOWERS) {
            return KEEP;
        }
    }
    if r == IS {
        // LB15c.
        if is(l, SPACES) {
            if pair.next == UNREAD {
                return NEEDS_NEXT;
            }
            if pair.next == NU {
                return BREAK;
            }
        }
        // LB15d.
        return KEEP;
    }
    // LB16.
    if (is(l, CLOSES) || l == SP_CLCP) && is(r, NONSTARTERS) {
        return KEEP;
    }
    // LB17.
    if (l == B2 || l == SP_B2) && r == B2 {
        return KEEP;
    }
    // LB18.
    if is(l, SPACES) {
        return BREAK;
    }
    // LB19.
    if legacy(rules, Legacy::LB19) {
        if is(l, QUOTES) || is(r, QUOTES) {
            return KEEP;
        }
    } else {
        if r == QU || r == QU_PF || l == QU || l == QU_PI || l == QU_PI_START {
            return KEEP;
        }
        // LB19a.
        if pair.flags_unread && (is(r, QUOTES) || is(l, QUOTES)) {
            return NEEDS_FLAGS;
        }
        if is(r, QUOTES) {
            if pair.flags & LEFT_EAST_ASIAN == 0 {
                return KEEP;
            }
            if pair.next == UNREAD {
                return NEEDS_NEXT;
            }
            if pair.next == EOT || pair.flags & NEXT_EAST_ASIAN == 0 {
                return KEEP;
            }
        }
        if is(l, QUOTES)
            && (pair.flags & RIGHT_EAST_ASIAN == 0 || pair.flags & BEFORE_EAST_ASIAN == 0)
        {
            return KEEP;
        }
    }
    // LB20.
    if l == CB || r == CB {
        return BREAK;
    }
    // LB20a.
    if !legacy(rules, Legacy::LB20A)
        && (l == HY_WORD || l == HH_WORD)
        && (is(r, LETTERS) || r == ID_LETTER)
    {
        return KEEP;
    }
    // LB21.
    if r == BA || r == HH || r == HY || is(r, NONSTARTERS) || l == BB {
        return KEEP;
    }
    // LB21a.
    if legacy(rules, Legacy::LB21A) {
        if l == HY_HL || l == HH_HL || is_ba_after_hl(l) {
            return KEEP;
        }
    } else if (l == HY_HL || l == HH_HL) && r != HL {
        return KEEP;
    }
    // LB21b.
    if (l == SY || l == SY_NU) && r == HL {
        return KEEP;
    }
    // LB22.
    if r == IN {
        return KEEP;
    }
    // LB23.
    if (is(l, LETTERS) && r == NU) || (l == NU && is(r, LETTERS)) {
        return KEEP;
    }
    // The East Asian PO and PR of Japanese and Chinese `loose` take part in
    // fewer of the rules below, as ICU4C's `$POX` and `$PRX` do; ICU4X reads
    // them as PO and PR.
    let icu = legacy(rules, Legacy::ICU_TAILORING);
    let (prefixes, postfixes) = if icu {
        (PREFIXES, POSTFIXES)
    } else {
        (set(&[PR]), set(&[PO]))
    };
    let (beside_letters, after_numbers) = if icu {
        (AFFIXES, AFFIXES)
    } else {
        (AFFIXES_BESIDE_LETTERS, AFFIXES_AFTER_NUMBERS)
    };
    // LB23a.
    if (is(l, prefixes) && is(r, IDEOGRAPHS)) || (is(l, IDEOGRAPHS) && is(r, postfixes)) {
        return KEEP;
    }
    // LB24.
    if (is(l, beside_letters) && is(r, LETTERS)) || (is(l, LETTERS) && is(r, beside_letters)) {
        return KEEP;
    }
    // LB25.
    if legacy(rules, Legacy::LB25) {
        if numbers_15_1(l, r) {
            return KEEP;
        }
    } else {
        if (l == CL_NU || l == CP_NU) && is(r, after_numbers) {
            return KEEP;
        }
        if is(l, NUMBER) && (is(r, after_numbers) || r == NU) {
            return KEEP;
        }
        if is(l, beside_letters) {
            if r == NU {
                return KEEP;
            }
            if is(r, OPENS) {
                if pair.next == UNREAD {
                    return NEEDS_NEXT;
                }
                if pair.next == NU {
                    return KEEP;
                }
                if pair.next == IS {
                    if pair.second == UNREAD {
                        return NEEDS_SECOND;
                    }
                    if pair.second == NU {
                        return KEEP;
                    }
                }
            }
        }
        if (is(l, HYPHENS) || l == IS || l == IS_NU) && r == NU {
            return KEEP;
        }
    }
    // LB26.
    if (l == JL && (r == JL || r == JV || r == H2 || r == H3))
        || ((l == JV || l == H2) && (r == JV || r == JT))
        || ((l == JT || l == H3) && r == JT)
    {
        return KEEP;
    }
    // LB27.
    if (is(l, KOREAN) && is(r, postfixes)) || (is(l, prefixes) && is(r, KOREAN)) {
        return KEEP;
    }
    // LB28.
    if is(l, LETTERS) && is(r, LETTERS) {
        return KEEP;
    }
    // LB28a.
    if (l == AP && is(r, AKSARA))
        || (is(l, AKSARA) && (r == VF || r == VI))
        || (l == VI_AK && (r == AK || r == DOTTED))
    {
        return KEEP;
    }
    if is(l, AKSARA) && is(r, AKSARA) {
        if pair.next == UNREAD {
            return NEEDS_NEXT;
        }
        if pair.next == VF {
            return KEEP;
        }
    }
    // LB29.
    if (l == IS || l == IS_NU) && is(r, LETTERS) {
        return KEEP;
    }
    // LB30.
    if ((is(l, LETTERS) || l == NU) && r == OP)
        || ((l == CP || l == CP_NU) && (is(r, LETTERS) || r == NU))
    {
        return KEEP;
    }
    // LB30a.
    if l == RI && r == RI {
        return KEEP;
    }
    // LB30b.
    if r == EM {
        if l == EB {
            return KEEP;
        }
        if pair.flags_unread {
            return NEEDS_FLAGS;
        }
        if pair.flags & LEFT_UNASSIGNED_PICTOGRAPH != 0 {
            return KEEP;
        }
    }
    // LB31.
    BREAK
}

/// Returns ICU4X's answer before the rules, where its tailorings come
/// first: a pair after a zero width joiner is held, and then the tailorings
/// apply unless a longer rule is partway through. Returns [`UNREAD`] where
/// the rules decide.
const fn icu_first(pair: &Pair, rules: &Rules) -> u8 {
    if pair.flags & AFTER_ZWJ != 0 {
        return KEEP;
    }
    if icu_inner(pair, rules) {
        return dotted_pair(pair, rules);
    }
    let tailored = tailor(pair, rules);
    if tailored != UNREAD {
        return tailored;
    }
    dotted_pair(pair, rules)
}

/// Returns whether one of ICU4X's tailorings, rather than its rules,
/// decides `pair`.
#[cfg(test)]
pub(super) const fn icu_tailors(pair: &Pair, rules: &Rules) -> bool {
    !icu_inner(pair, rules) && tailor(pair, rules) != UNREAD
}

/// Returns ICU4X's answer after two U+25CC, which holds only a letter, a
/// non-East Asian OP, VF and an initial quotation mark; [`UNREAD`] elsewhere
/// and outside tests.
const fn dotted_pair(pair: &Pair, rules: &Rules) -> u8 {
    #[cfg(test)]
    if legacy(rules, Legacy::DOTTED_PAIRS) && pair.left == DOTTED_PAIR {
        let held = set(&[AI, AL, HL, OP, SA, VF, XX, QU_PI]);
        return if is(pair.right, held) { KEEP } else { BREAK };
    }
    let _ = (pair, rules);
    UNREAD
}

/// Returns whether ICU4X's segmenter is partway through a longer rule at
/// `pair`, where it reads no tailoring; never outside tests.
#[inline(always)]
const fn icu_inner(pair: &Pair, rules: &Rules) -> bool {
    #[cfg(test)]
    {
        legacy(rules, Legacy::TAILORING_OUTSIDE_RULES)
            && (pair.flags & ICU_INNER != 0
                || (pair.flags & ICU_INNER_BEFORE_FOLLOWER != 0 && is(pair.right, QUOTE_FOLLOWERS)))
    }
    #[cfg(not(test))]
    {
        let _ = (pair, rules);
        false
    }
}

/// Returns the flags that say whether ICU4X's segmenter is partway through a
/// longer rule after a character of class `right` takes `left` to `next`.
#[cfg(test)]
pub(super) const fn icu_inner_flags(left: u8, right: u8, next: u8) -> u8 {
    let inner = set(&[
        SP_OP,
        SP_QU,
        SP_CLCP,
        SP_B2,
        SP_ZW,
        QU_PI,
        HY_HL,
        HH_HL,
        BA_HL,
        RI_EVEN,
        VI_AK,
        DOTTED_PAIR,
    ]);
    if is(next, inner) || (right == QU_PF && (left == SP_OP || left == CB)) {
        ICU_INNER
    } else if right == QU_PF && is(left, SPACES) {
        ICU_INNER_BEFORE_FOLLOWER
    } else {
        0
    }
}

/// Returns whether `left` is BA after HL, which Unicode 15.1's LB21a holds;
/// never outside tests.
#[inline(always)]
const fn is_ba_after_hl(left: u8) -> bool {
    #[cfg(test)]
    {
        left == BA_HL
    }
    #[cfg(not(test))]
    {
        let _ = left;
        false
    }
}

/// Returns whether Unicode 15.1's LB25 holds `left` and `right` together:
/// `(CL | CP | NU) × (PO | PR)`, `(PO | PR) × (OP | NU)`, `(HY | IS | NU |
/// SY) × NU`.
const fn numbers_15_1(left: u8, right: u8) -> bool {
    let closes = is(left, CLOSES);
    let numbers = left == NU;
    let affixes = is(left, AFFIXES);
    ((closes || numbers) && is(right, AFFIXES))
        || (affixes && (is(right, OPENS) || right == NU))
        || ((is(left, HYPHENS)
            || left == IS
            || left == IS_NU
            || numbers
            || left == SY
            || left == SY_NU)
            && right == NU)
}

/// Returns the options' answer for `pair`, or [`UNREAD`] where they leave it
/// to the rules.
///
/// - `keep-all` holds letters, digits, ideographs, Hangul and CJ together.
/// - `loose` breaks between ellipses (IN), as ICU4C's `line_loose.txt` does.
/// - Japanese and Chinese `loose` breaks before U+2010 HYPHEN and U+2013 EN
///   DASH after an ideograph, as ICU4C's `line_loose_cj.txt` does (`漢|‐`,
///   `漢|–`), and so does `loose` with `break-all` in
///   any language, where letters and digits read as ideographs (`a|‐a`).
///
/// The characters a strictness lets a line break before or after are
/// classes of their own ([`NSX`], [`PO_EAW`], [`PR_EAW`]), not tailorings
/// here: a rule that holds before any character holds before them too (`(〜`, `"〜`, `´〜`).
const fn tailor(pair: &Pair, rules: &Rules) -> u8 {
    let (l, r) = (pair.left, pair.right);
    let keep_all = if legacy(rules, Legacy::KEEP_ALL_HYPHENS) {
        icu_keep_all()
    } else {
        KEEP_ALL
    };
    if rules.keep_all && is(l, keep_all) && is(r, keep_all) {
        return KEEP;
    }
    if legacy(rules, Legacy::ICU_TAILORING) {
        return icu_tailor(pair, rules);
    }
    if matches!(rules.tailoring, Tailoring::Loose)
        && ((l == IN && r == IN)
            || (rules.loose_hyphens && is(l, LOOSE_HYPHEN_BASES) && is_loose_hyphen(pair.ch)))
    {
        return BREAK;
    }
    UNREAD
}

/// Returns whether `ch` is a hyphen Japanese and Chinese `loose` breaks
/// before after an ideograph: U+2010 HYPHEN or U+2013 EN DASH.
#[inline(always)]
const fn is_loose_hyphen(ch: char) -> bool {
    matches!(ch, '\u{2010}' | '\u{2013}')
}

/// Returns ICU4X's `keep-all` set; never outside tests.
#[inline(always)]
const fn icu_keep_all() -> Set {
    #[cfg(test)]
    {
        ICU_KEEP_ALL
    }
    #[cfg(not(test))]
    {
        KEEP_ALL
    }
}

/// Returns ICU4X's answer for `pair` under its strictness tailorings, or
/// [`UNREAD`] where they leave it to the rules; never outside tests.
///
/// - Japanese and Chinese `normal` breaks before U+301C `〜` and U+30A0 `゠`.
/// - `loose` breaks before U+2010 HYPHEN and U+2013 EN DASH after an
///   ideograph, before iteration marks and between ellipses; with Japanese
///   and Chinese, also before `〜` and `゠`, before centred punctuation and
///   fullwidth `!` and `?`, before an East Asian postfix and after an East
///   Asian prefix.
const fn icu_tailor(pair: &Pair, rules: &Rules) -> u8 {
    #[cfg(test)]
    {
        let (l, r) = (pair.left, pair.right);
        let ch = pair.ch as u32;
        // A tailoring this option set leaves out: ICU4X holds the pair, and
        // the rules decide it otherwise.
        let off = if legacy(rules, Legacy::LOOSE_HOLDS) {
            KEEP
        } else {
            UNREAD
        };
        let ja_zh = if rules.ja_zh { BREAK } else { off };
        match rules.tailoring {
            Tailoring::None => UNREAD,
            Tailoring::NormalJapaneseChinese => {
                if ch == 0x301C || ch == 0x30A0 {
                    BREAK
                } else {
                    UNREAD
                }
            }
            Tailoring::Loose => {
                // Each pair matches one of these at most, except a prefix
                // before an iteration mark, which breaks either way.
                let always = ((ch == 0x2010 || ch == 0x2013) && (l == ID || l == ID_LETTER))
                    || (r == NS
                        && matches!(ch, 0x3005 | 0x303B | 0x309D | 0x309E | 0x30FD | 0x30FE))
                    || (r == IN && l == IN);
                let japanese_chinese = (r == NS
                    && matches!(
                        ch,
                        0x301C | 0x30A0 | 0x30FB | 0xFF1A | 0xFF1B | 0xFF65 | 0x203C | 0x2047
                            ..=0x2049
                    ))
                    || (r == EX && (ch == 0xFF01 || ch == 0xFF1F))
                    || r == PO_EAW
                    || l == PR_EAW;
                if always {
                    BREAK
                } else if japanese_chinese {
                    ja_zh
                } else {
                    UNREAD
                }
            }
        }
    }
    #[cfg(not(test))]
    {
        let _ = (pair, rules);
        UNREAD
    }
}

/// Returns the state after a character `ch` of class `right` that follows
/// `left`, where the character is not a mark joining the one before it.
///
/// A hyphen that Japanese and Chinese `loose` breaks before after an
/// ideograph starts a word (LB20a), as ICU4C's rules read a hyphen after
/// any break: `漢|‐a` holds after the hyphen.
pub(super) const fn next_left(left: u8, right: u8, ch: char, rules: &Rules) -> u8 {
    match right {
        SP => {
            if left == ZW || left == SP_ZW {
                SP_ZW
            } else if is(left, OPENS) || left == SP_OP {
                SP_OP
            } else if left == QU_PI_START || left == SP_QU {
                SP_QU
            } else if is(left, CLOSES) || left == SP_CLCP {
                SP_CLCP
            } else if left == B2 || left == SP_B2 {
                SP_B2
            } else {
                SP
            }
        }
        QU_PI if is(left, QUOTE_OPENERS) => QU_PI_START,
        HY | HH => {
            let word = is(left, WORD_STARTS)
                || (rules.loose_hyphens
                    && !legacy(rules, Legacy::ICU_TAILORING)
                    && is(left, LOOSE_HYPHEN_BASES)
                    && is_loose_hyphen(ch));
            match (right, word, left == HL) {
                (HY, true, _) => HY_WORD,
                (HY, _, true) => HY_HL,
                (HH, true, _) => HH_WORD,
                (HH, _, true) => HH_HL,
                _ => right,
            }
        }
        SY if is(left, NUMBER) => SY_NU,
        IS if is(left, NUMBER) => IS_NU,
        CL if is(left, NUMBER) => CL_NU,
        CP if is(left, NUMBER) => CP_NU,
        VI if is(left, AKSARA) => VI_AK,
        RI if left == RI => RI_EVEN,
        BA if left == HL && legacy(rules, Legacy::LB21A) => ba_after_hl(),
        DOTTED if left == DOTTED && legacy(rules, Legacy::DOTTED_PAIRS) => dotted_after_dotted(),
        _ => right,
    }
}

/// Returns the state of BA after HL under Unicode 15.1's LB21a.
#[inline(always)]
const fn ba_after_hl() -> u8 {
    #[cfg(test)]
    {
        BA_HL
    }
    #[cfg(not(test))]
    {
        BA
    }
}

/// Returns the state of U+25CC after U+25CC under ICU4X's quirk.
#[inline(always)]
const fn dotted_after_dotted() -> u8 {
    #[cfg(test)]
    {
        DOTTED_PAIR
    }
    #[cfg(not(test))]
    {
        DOTTED
    }
}

/// The number of strictnesses the resolve tables are for: `strict` (and
/// `anywhere`), `normal`, Japanese and Chinese `normal`, `loose`, and
/// Japanese and Chinese `loose`, indexed in that order.
pub(super) const RESOLVE_STRICTNESSES: usize = 5;

/// Returns the class `raw` resolves to (LB1) under the options.
///
/// - `strictness` is an index as [`RESOLVE_STRICTNESSES`] counts them. All
///   but `strict` read CJ as ID, and each makes the characters it lets a
///   line break before [`NSX`]. The East Asian PO and PR keep classes of
///   their own under Japanese and Chinese `loose` only.
/// - `break_all` reads AL, NU and SA as ID, as ICU4X's segmenter does.
/// - SA that is a mark is CM, or with `sa_marks` SA, as ICU4X's segmenter
///   reads it.
/// - `icu` resolves as ICU4X's segmenter does, which tailors by character
///   rather than by class.
const fn resolve(raw: u8, strictness: usize, break_all: bool, sa_marks: bool, icu: bool) -> u8 {
    let cj_as_id = strictness != 0;
    let normal_ja_zh = strictness == 2;
    let loose = strictness >= 3;
    let loose_ja_zh = strictness == 4;
    match raw {
        SA_MARK if sa_marks && break_all => ID,
        SA_MARK if sa_marks => SA,
        SA_MARK => CM,
        CJ if cj_as_id || break_all => ID,
        AL if break_all => ID_LETTER,
        NU | SA if break_all => ID,
        NS_DASH if !icu && (normal_ja_zh || loose_ja_zh) => NSX,
        NS_ITERATION if !icu && loose => NSX,
        NS_CENTRED | EX_WIDE if !icu && loose_ja_zh => NSX,
        NS_DASH | NS_ITERATION | NS_CENTRED => NS,
        EX_WIDE => EX,
        PO_EAW if !icu && !loose_ja_zh => PO,
        PR_EAW if !icu && !loose_ja_zh => PR,
        _ => raw,
    }
}

/// Returns the table of [`resolve`] for each class, under the options.
const fn resolve_table(strictness: usize, break_all: bool, sa_marks: bool, icu: bool) -> [u8; 64] {
    let mut table = [XX; 64];
    let mut raw = 0;
    while raw < DATA_CLASS_COUNT {
        table[raw] = resolve(raw as u8, strictness, break_all, sa_marks, icu);
        raw += 1;
    }
    table
}

/// Returns [`resolve`]'s tables, by strictness and then `break_all`.
const fn resolve_tables(sa_marks: bool, icu: bool) -> [[u8; 64]; 2 * RESOLVE_STRICTNESSES] {
    let mut tables = [[XX; 64]; 2 * RESOLVE_STRICTNESSES];
    let mut index = 0;
    while index < tables.len() {
        tables[index] = resolve_table(index / 2, index % 2 == 1, sa_marks, icu);
        index += 1;
    }
    tables
}

/// [`resolve`]'s tables, by strictness and then `break_all`.
pub(super) static RESOLVE: [[u8; 64]; 2 * RESOLVE_STRICTNESSES] = resolve_tables(false, false);

/// [`resolve`]'s tables with SA marks read as ICU4X's segmenter reads them,
/// by whether its tailorings apply too, then by strictness and `break_all`.
#[cfg(test)]
pub(super) static RESOLVE_ICU: [[[u8; 64]; 2 * RESOLVE_STRICTNESSES]; 4] = [
    resolve_tables(false, false),
    resolve_tables(true, false),
    resolve_tables(false, true),
    resolve_tables(true, true),
];

/// The number of states the pair table has a row for: every class, and the
/// states that read a context, up to [`RI_EVEN`].
pub(super) const TABLE_STATES: usize = RI_EVEN as usize + 1;

/// The bit of a [`PAIRS`] entry that says the pair breaks.
pub(super) const PAIR_BREAKS: u8 = 1;
/// The bit of a [`PAIRS`] entry that says the pair breaks under `keep-all`.
pub(super) const PAIR_BREAKS_KEEP_ALL: u8 = 2;
/// A [`PAIRS`] entry whose answer [`decide`] gives only with more than the
/// two classes: the East Asian or pictograph flags, or lookahead.
pub(super) const PAIR_READS_MORE: u8 = 4;

/// [`decide`]'s answer for each state and class, where it depends on
/// nothing else, under no tailoring but `keep-all`.
///
/// Built from [`decide`] itself, with the East Asian and pictograph flags
/// unread: an entry for which it asks for them, or for lookahead, is
/// [`PAIR_READS_MORE`]. The scan reads an entry only where
/// no zero width joiner precedes the boundary and no strictness tailoring
/// could apply ([`may_tailor`]).
pub(super) static PAIRS: [[u8; CLASS_COUNT]; TABLE_STATES] = {
    let mut table = [[0; CLASS_COUNT]; TABLE_STATES];
    let mut left = 0;
    while left < TABLE_STATES {
        let mut right = 0;
        while right < CLASS_COUNT {
            table[left][right] = pair_entry(left as u8, right as u8);
            right += 1;
        }
        left += 1;
    }
    table
};

/// Returns the [`PAIRS`] entry of `left` and `right`.
const fn pair_entry(left: u8, right: u8) -> u8 {
    let mut entry = 0;
    let mut keep_all = 0;
    while keep_all < 2 {
        let rules = Rules {
            tailoring: Tailoring::None,
            ja_zh: false,
            keep_all: keep_all == 1,
            loose_hyphens: false,
            #[cfg(test)]
            legacy: 0,
        };
        let pair = Pair {
            left,
            right,
            flags: 0,
            flags_unread: true,
            ch: '\0',
            next: UNREAD,
            second: UNREAD,
        };
        let answer = decide(&pair, &rules);
        if answer != KEEP && answer != BREAK {
            return PAIR_READS_MORE;
        }
        entry |= answer << keep_all;
        keep_all += 1;
    }
    entry
}

/// Returns whether `loose` may decide a boundary between `left` and
/// `right` ([`tailor`]), so the pair table does not answer for it.
#[inline(always)]
pub(super) const fn may_tailor(left: u8, right: u8) -> bool {
    (left == IN && right == IN) || (is(left, LOOSE_HYPHEN_BASES) && (right == HH || right == BA))
}
