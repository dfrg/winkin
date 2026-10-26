//! A run of characters whose words a dictionary finds, as a forward scan
//! meets it.
//!
//! Line and word segmentation share it. Inside a run, a boundary falls only
//! where the next word starts; where the run's script changes, it falls if
//! either script has a dictionary; where a run starts or ends, the
//! segmenter's rules decide.

use super::matcher::longest_word;
use super::{Dictionaries, Dictionary, DictionaryScript, next_word};
use crate::{GraphemeProps, GraphemeState, ScriptId, core_props};

/// The script of a run a dictionary segments, which picks its dictionary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum RunScript {
    Thai,
    Lao,
    Khmer,
    Myanmar,
    /// Han, Hiragana and Katakana.
    ChineseJapanese,
    /// A Southeast Asian script with no dictionary, such as Tai Tham.
    Other,
}

impl RunScript {
    /// Returns the run script of `ch`, a Line_Break SA character.
    #[inline]
    pub(crate) fn from_sa_char(ch: char) -> Self {
        match core_props(ch).script() {
            ScriptId::THAI => Self::Thai,
            ScriptId::LAO => Self::Lao,
            ScriptId::KHMER => Self::Khmer,
            ScriptId::MYANMAR => Self::Myanmar,
            _ => Self::Other,
        }
    }

    /// Returns the dictionary script of the run, if it can have one.
    pub(crate) fn dictionary(self) -> Option<DictionaryScript> {
        Some(match self {
            Self::Thai => DictionaryScript::Thai,
            Self::Lao => DictionaryScript::Lao,
            Self::Khmer => DictionaryScript::Khmer,
            Self::Myanmar => DictionaryScript::Myanmar,
            Self::ChineseJapanese => DictionaryScript::ChineseJapanese,
            Self::Other => return None,
        })
    }

    /// Returns the dictionary of the run in `dictionaries`, if there is one.
    #[inline]
    pub(crate) fn find(self, dictionaries: Option<&Dictionaries>) -> Option<&Dictionary> {
        dictionaries?.get(self.dictionary()?)
    }
}

/// Where a scan stands in a run: the run's script, and where its next word
/// starts.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Run {
    /// The script of the run the last character is in, or `None` outside
    /// one.
    script: Option<RunScript>,
    /// The offset of the next word of the run, or `None` where no word
    /// starts before the run ends.
    next_word: Option<usize>,
}

impl Run {
    /// Returns the script of the run the last character is in, or `None`
    /// outside one.
    #[inline]
    pub(crate) fn script(&self) -> Option<RunScript> {
        self.script
    }

    /// Ends the run, at a character outside any.
    #[inline]
    pub(crate) fn end(&mut self) {
        self.script = None;
    }

    /// Moves the run past the character at `offset`, which is in a run of
    /// `script`, and a mark where `mark`. Returns whether a boundary falls
    /// before it where the run decides, or `None` where the rules do.
    ///
    /// `find` returns where the word after the one starting at the
    /// character ends, by the dictionary it is given. A mark starts a run
    /// but never a word.
    #[inline]
    pub(crate) fn step(
        &mut self,
        offset: usize,
        script: RunScript,
        mark: bool,
        dictionaries: Option<&Dictionaries>,
        find: impl FnOnce(&Dictionary) -> Option<usize>,
    ) -> Option<bool> {
        let dictionary = script.find(dictionaries);
        let previous = self.script.replace(script);
        if previous == Some(script) {
            if self.next_word != Some(offset) {
                return Some(false);
            }
            self.next_word = dictionary.and_then(find);
            return Some(true);
        }
        self.next_word = dictionary.and_then(find);
        // A word of a dictionary ends where its script does.
        let previous = previous?;
        (dictionary.is_some() || previous.find(dictionaries).is_some()).then_some(!mark)
    }
}

/// The longest run of katakana that ICU4C's Chinese and Japanese search may
/// take as one word (`kMaxKatakanaLength`).
const KATAKANA_WORD: usize = 8;

/// Returns whether `ch` is katakana as ICU4C's Chinese and Japanese search
/// reads it (`isKatakana`): U+30A1 to U+30FE but the middle dot U+30FB, the
/// prolonged sound mark and the iteration marks included, and the halfwidth
/// forms.
#[inline]
pub(crate) fn is_katakana(ch: char) -> bool {
    matches!(u32::from(ch), 0x30A1..=0x30FA | 0x30FC..=0x30FE | 0xFF66..=0xFF9F)
}

/// Returns where the word after the one starting with `ch`, at `offset`,
/// starts in a Chinese and Japanese run, as [`find_next_word`] does, but
/// for a short run of katakana.
///
/// Where a run of katakana starts at `ch`, after no katakana
/// (`katakana_start`), and is at most [`KATAKANA_WORD`] long, it is one word
/// unless the dictionary's longest words cover it with no miss. ICU4C's
/// search gives such a run a cost by its length (`getKatakanaCost`), so
/// that a word the dictionary lacks stays whole: `グーグルマップ` stays whole
/// and `ゲーム|ソフト` parts.
#[expect(clippy::too_many_arguments, reason = "find_next_word's, and one")]
pub(crate) fn find_next_cj_word<I: Iterator<Item = (usize, char)> + Clone>(
    dictionary: &Dictionary,
    offset: usize,
    ch: char,
    rest: I,
    continues: impl Fn(char) -> bool + Copy,
    props: impl Fn(char) -> GraphemeProps + Copy,
    graphemes: GraphemeState,
    katakana_start: bool,
) -> Option<usize> {
    if katakana_start && is_katakana(ch) {
        // The katakana's end: the offset of the first character after it, or
        // `None` where it reaches the run's end.
        let mut end = None;
        let mut length = 1;
        let mut least = offset.saturating_add(1);
        for (at, next) in rest.clone() {
            if at < least || !continues(next) || !is_katakana(next) {
                end = (at >= least).then_some(at);
                break;
            }
            least = at.saturating_add(1);
            length += 1;
            if length > KATAKANA_WORD {
                break;
            }
        }
        if length <= KATAKANA_WORD {
            let first = find_cj_words(
                dictionary, offset, ch, rest, continues, props, graphemes, end,
            );
            if let Some(first) = first {
                return first;
            }
            return end;
        }
    }
    find_next_word(dictionary, offset, ch, rest, continues, props, graphemes)
}

/// Returns `Some` with where the first word starts after the one at `ch`,
/// at `offset`, where the dictionary's longest words cover the katakana up
/// to `end` with no miss, or `None` where one misses before it. `end` is
/// `None` where the katakana reaches the run's end.
#[expect(clippy::too_many_arguments, reason = "find_next_word's, and one")]
fn find_cj_words<I: Iterator<Item = (usize, char)> + Clone>(
    dictionary: &Dictionary,
    offset: usize,
    ch: char,
    rest: I,
    continues: impl Fn(char) -> bool + Copy,
    props: impl Fn(char) -> GraphemeProps + Copy,
    graphemes: GraphemeState,
    end: Option<usize>,
) -> Option<Option<usize>> {
    let inside = |at: usize| end.is_none_or(|end| at < end);
    let mut first = None;
    let (mut at, mut start) = (offset, ch);
    loop {
        let mut least = at.saturating_add(1);
        let chars = core::iter::once((at, start)).chain(
            rest.clone()
                .skip_while(move |&(next, _)| next <= at)
                .take_while(move |&(next, ch)| {
                    let inside = next >= least && continues(ch);
                    least = next.saturating_add(1);
                    inside
                }),
        );
        let (next, found) = longest_word(dictionary, chars, props, graphemes);
        if !found {
            return None;
        }
        let first = *first.get_or_insert(next);
        match next {
            Some(next) if inside(next) => {
                let Some((_, ch)) = rest.clone().find(|&(found, _)| found == next) else {
                    return Some(first);
                };
                (at, start) = (next, ch);
            }
            _ => return Some(first),
        }
    }
}

/// Returns where the word after the one starting with `ch`, at `offset`,
/// starts in its run, or `None` where none does before the run ends.
///
/// `rest` holds the characters after `ch`; the run goes on while their
/// offsets increase and `continues` holds. `props` gives each character's
/// grapheme properties and `graphemes` is the grapheme state at a text's
/// start.
pub(crate) fn find_next_word(
    dictionary: &Dictionary,
    offset: usize,
    ch: char,
    rest: impl Iterator<Item = (usize, char)>,
    continues: impl Fn(char) -> bool,
    props: impl Fn(char) -> GraphemeProps,
    graphemes: GraphemeState,
) -> Option<usize> {
    let mut least = offset.saturating_add(1);
    let rest = rest.take_while(move |&(at, next)| {
        let inside = at >= least && continues(next);
        least = at.saturating_add(1);
        inside
    });
    next_word(
        dictionary,
        core::iter::once((offset, ch)).chain(rest),
        props,
        graphemes,
    )
}
