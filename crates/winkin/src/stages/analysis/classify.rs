//! Per-character classifiers for the walk: a cluster's class, the Latin-1
//! fast path, hanging, glue, the East Asian gate and vertical orientation.

use super::scripts::{is_shared, is_vertical_only};
use super::walk::ItemInfo;
use super::{ClusterClass, RunOrientation};
#[cfg(test)]
use crate::stages::content::Content;
use crate::stages::content::{ItemKind, TextSetting};
use crate::unicode;
use crate::unicode::BidiClass;
use crate::unicode::{CoreProps, RareProps};

/// How a style's text stands in the block's lines, as the builder lowered it,
/// asked of each cluster.
impl TextSetting {
    /// Returns how a cluster whose first character is `ch` stands, set so.
    ///
    /// - **`mixed`** reads the character's Vertical_Orientation, as Blink's
    ///   `OrientationIterator` does. It reads a cluster at a time, where Blink
    ///   reads a character with its grapheme extenders. So a ZWJ sequence or a
    ///   keycap stands as its first character does, as CSS Writing Modes 3,
    ///   section 5.1.1, has a grapheme stand.
    /// - **`upright`** keeps a vertical-only script's characters (Mongolian,
    ///   Phags-pa) on their side, as CSS Writing Modes 4 says. Blink sets them
    ///   upright too. A character of no script of its own stands upright
    ///   here, and the runs set it as its script run stands
    ///   ([`follows_script_run`](Self::follows_script_run)).
    ///
    /// `rare` is the character's rare word where the writer read it, which it
    /// does past Latin-1.
    pub(super) fn orientation(
        self,
        ch: char,
        props: CoreProps,
        rare: Option<RareProps>,
    ) -> RunOrientation {
        let upright = || {
            rare.unwrap_or_else(|| unicode::rare_props(ch))
                .is_mixed_upright()
        };
        match self {
            Self::Horizontal => RunOrientation::Horizontal,
            Self::Sideways => RunOrientation::Sideways,
            Self::Mixed if upright() => RunOrientation::Upright,
            Self::Mixed => RunOrientation::Sideways,
            Self::Upright if is_vertical_only(unicode::script(props)) => RunOrientation::Sideways,
            Self::Upright => RunOrientation::Upright,
        }
    }

    /// Returns how generated text `text`, a hyphen or an ellipsis, stands
    /// in a style set so.
    ///
    /// It follows the text it ends, as Blink shapes a hyphen in its style's
    /// font orientation (`HyphenResult::Shape`). Under `mixed` it stands
    /// upright only where every character does, so a hyphen goes on its side.
    pub(crate) fn generated_orientation(self, text: &str) -> RunOrientation {
        match self {
            Self::Horizontal => RunOrientation::Horizontal,
            Self::Sideways => RunOrientation::Sideways,
            Self::Upright => RunOrientation::Upright,
            Self::Mixed
                if text
                    .chars()
                    .all(|ch| unicode::rare_props(ch).is_mixed_upright()) =>
            {
                RunOrientation::Upright
            }
            Self::Mixed => RunOrientation::Sideways,
        }
    }

    /// Whether a cluster whose first character's core word is `props` stands as
    /// the script run it resolves into, set so.
    ///
    /// Under `upright`, a Common, Inherited or Unknown character is of the
    /// script run around it, so a space between Mongolian words goes on its
    /// side with them, and one between Latin words stands up. Blink stands
    /// every such character up.
    pub(super) fn follows_script_run(self, props: CoreProps) -> bool {
        self == Self::Upright && is_shared(unicode::script(props))
    }
}

/// Returns a cluster's class from its first character and its item.
///
/// A U+200B is a break opportunity where the builder generated it, and text
/// where the caller wrote it. A mandatory break is drawn where it is a
/// control character. `props` and `rare` are the character's words; the writer
/// reads the rare one past Latin-1.
pub(super) fn first_class(
    ch: char,
    props: CoreProps,
    rare: Option<RareProps>,
    item: &ItemInfo,
) -> ClusterClass {
    if item.kind == ItemKind::Atomic {
        return ClusterClass::Object;
    }
    if props.is_paragraph_separator() {
        // VT, FF and NEL, the controls among them, are drawn visibly (CSS
        // Text 3, section 4). A lone CR never reaches the text: the builder
        // writes it as a space.
        return if matches!(ch, '\u{B}' | '\u{C}' | '\u{85}') {
            ClusterClass::DrawnSeparator
        } else {
            ClusterClass::Separator
        };
    }
    match ch {
        ' ' => ClusterClass::Space,
        '\t' => ClusterClass::Tab,
        '\u{AD}' => ClusterClass::SoftHyphen,
        '\u{200B}' if item.generated => ClusterClass::BreakOpportunity,
        '\u{200B}' => ClusterClass::ZeroWidthSpace,
        // What is left of White_Space is the space separators (Zs): the
        // controls and U+2028/9 are separators or tabs, above. The glue among
        // them is GL, the no-break spaces: none is WJ or ZWJ.
        _ if props.is_white_space() => {
            if rare.unwrap_or_else(|| unicode::rare_props(ch)).is_glue() {
                ClusterClass::NoBreakSpace
            } else {
                ClusterClass::OtherSpace
            }
        }
        _ if props.is_emoji_presentation() => ClusterClass::Emoji,
        _ if props.is_extended_pictographic() => ClusterClass::Symbol,
        // Every default-ignorable character but the soft hyphen is past
        // Latin-1 (the sweep holds it), so Latin-1 reads no rare word here.
        _ if rare.is_some_and(RareProps::is_default_ignorable) => ClusterClass::Control,
        _ => ClusterClass::Text,
    }
}

/// Whether `ch` is a fast-path character: printable Latin-1 but the soft hyphen.
///
/// The fast path is U+0020 to U+007E and U+00A0 to U+00FF. Each fast-path character
/// stands alone in grapheme clustering and is a cluster of its own class
/// ([`latin1_fast_class`]). None is a separator, a tab or a control.
#[inline]
pub(super) fn is_latin1_fast(ch: char) -> bool {
    matches!(ch, ' '..='~' | '\u{A0}'..='\u{FF}') && ch != '\u{AD}'
}

/// Returns the fast-path character at byte `at` of `bytes` and its length.
///
/// `bytes` is valid UTF-8. A fast-path character is an ASCII byte or a two-byte
/// sequence led by 0xC2 or 0xC3. Returns `None` where the text ends or the
/// character is not a fast-path character.
#[inline]
pub(super) fn latin1_fast_char(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let lead = *bytes.get(at)?;
    let (ch, len) = if lead < 0x80 {
        (char::from(lead), 1)
    } else if lead & 0xFE == 0xC2 {
        let trail = *bytes.get(at + 1)?;
        (char::from(((lead & 0x03) << 6) | (trail & 0x3F)), 2)
    } else {
        return None;
    };
    is_latin1_fast(ch).then_some((ch, len))
}

/// Returns the class of a text item's cluster that is the fast-path character `ch`.
///
/// It gives [`first_class`]'s answer; a test checks every fast-path character.
#[inline]
pub(super) fn latin1_fast_class(ch: char) -> ClusterClass {
    match ch {
        ' ' => ClusterClass::Space,
        '\u{A0}' => ClusterClass::NoBreakSpace,
        // Extended_Pictographic, in text presentation.
        '\u{A9}' | '\u{AE}' => ClusterClass::Symbol,
        _ => ClusterClass::Text,
    }
}

/// Whether the fast path takes `ch` as the general walk does, in a text item.
///
/// The test sweeps this. Such a character is a cluster of [`latin1_fast_class`] and
/// stands alone in grapheme clustering. It is no separator and no held small
/// kana, and it is not East Asian. Its bidi class keeps a cluster at its
/// paragraph's level.
#[cfg(test)]
pub(super) fn latin1_fast_agrees(ch: char, content: &Content) -> bool {
    let item = ItemInfo::from_block(content);
    let props = unicode::core_props(ch);
    let rare = unicode::rare_props(ch);
    is_latin1_fast(ch)
        && first_class(ch, props, None, &item) == latin1_fast_class(ch)
        && rare.grapheme_cluster_break().stands_alone()
        && !props.is_paragraph_separator()
        && !rare.is_conditional_japanese_starter()
        && props.bidi_class().mask() & LEVEL_CLASSES == 0
        && !is_east_asian(ch)
}

/// Whether `ch` keeps a line from breaking beside an atomic inline.
///
/// Those are GL other than U+00A0, WJ and ZWJ (CSS Text 3, section 5.1).
pub(super) fn glues(ch: char) -> bool {
    ch != '\u{A0}' && unicode::rare_props(ch).is_glue()
}

/// Whether a cluster of `class` in `item` hangs at a line's end.
///
/// Hangs:
/// - spaces, tabs and the other space separators, unless `break-spaces` makes
///   them content or `pre` keeps them where the text does not wrap;
/// - a separator that draws nothing, so what hangs before a forced break
///   hangs;
/// - a break opportunity the builder generated (a `<wbr>`'s U+200B, or one
///   collapsing kept). It takes no room and is opaque to collapsing, so what
///   hangs before it hangs through it. Blink's line breaker and hanging pass
///   step over such items (`ComputeTrailingCollapsibleSpaceHelper`,
///   `ComputeTrailingSpaceWidth`).
///
/// Doesn't hang:
/// - a U+200B the caller wrote, which is text, so what is before it doesn't
///   hang either;
/// - a no-break space;
/// - a separator drawn as a glyph. It takes room, so the white space before
///   it stays inside the line.
///
/// The breaker decides, from what ends the line, whether a preserved space
/// hangs only conditionally.
pub(super) fn hangs(class: ClusterClass, item: &ItemInfo) -> bool {
    if class.is_breaking_space() {
        item.white_space_hangs
    } else {
        matches!(
            class,
            ClusterClass::Separator | ClusterClass::BreakOpportunity
        )
    }
}

/// Whether `ch` is East Asian for the paragraph's gate: Han, kana, Hangul,
/// Bopomofo, Yi and Tangut, their punctuation, and the fullwidth and
/// compatibility forms. CJK spacing and line-edge rules apply to these.
/// Emoji are not East Asian, so an emoji alone leaves the gate closed.
///
/// Every character `text-autospace` counts as an ideograph (UTR #59's wide
/// ones) is here, so a paragraph the gate leaves closed has no autospace
/// seam: the Bopomofo tone marks from U+02C7 to U+02EB among them, which are
/// modifier letters of the Common script (a test holds the gate to the
/// table).
pub(super) fn is_east_asian(ch: char) -> bool {
    ch >= '\u{2C7}'
        && matches!(
            ch,
            '\u{2C7}'..='\u{2EB}'
                | '\u{1100}'..='\u{11FF}'
                | '\u{2E80}'..='\u{A4CF}'
                | '\u{A960}'..='\u{A97F}'
                | '\u{AC00}'..='\u{D7FF}'
                | '\u{F900}'..='\u{FAFF}'
                | '\u{FE10}'..='\u{FE1F}'
                | '\u{FE30}'..='\u{FE4F}'
                | '\u{FF00}'..='\u{FFEF}'
                | '\u{16FE0}'..='\u{18DFF}'
                | '\u{1AFF0}'..='\u{1B2FF}'
                | '\u{20000}'..='\u{3FFFF}'
        )
}

/// The bidi classes that may take a cluster of a left-to-right paragraph
/// off level 0, as bits of a mask of classes: those that read right to
/// left, R, AN and AL, and the right-to-left controls, RLE, RLO and RLI.
///
/// The rest raise only even levels, which Blink drops for level 0.
pub(super) const LEVEL_CLASSES: u32 = BidiClass::RIGHT_TO_LEFT.mask()
    | BidiClass::ARABIC_NUMBER.mask()
    | BidiClass::ARABIC_LETTER.mask()
    | BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask();
