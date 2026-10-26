//! Word boundary tests:
//! - `conformance`: Unicode's WordBreakTest.txt, the word byte against the
//!   UCD, restarts, the POSIX full stop, streams that misbehave, and scans
//!   that allocate nothing;
//! - `differential`: the differential against ICU4X 2.3's word segmenter,
//!   whose data is Unicode 17.0's: under 17.0's data and a model of ICU4X's
//!   complex runs the two agree exactly on every pair and triple of word
//!   classes, the corpora and the script seams; under 18.0 each difference
//!   is counted by its cause.

mod conformance;
mod differential;

use std::vec::Vec;

use super::*;
use crate::tests::{RunQuirks, icu_run_breaks};

/// What the tests read in place of the crate's data and rules, and the
/// state of the model of ICU4X's complex runs.
#[derive(Clone, Debug, Default)]
pub(super) struct TestState {
    /// The word bytes of an older Unicode version.
    pub(super) classes: Option<&'static [u8]>,
    /// The grapheme properties of an older Unicode version.
    pub(super) graphemes: Option<&'static [GraphemeProps]>,
    /// Whether graphemes follow Unicode 17.0's GB9c.
    pub(super) graphemes_17: bool,
    /// The [`Legacy`] switches.
    pub(super) legacy: u32,
    /// The boundaries still ahead in the run the model segmented, with the
    /// run's end where they cover the run to its end.
    model: Option<(Vec<usize>, Option<usize>)>,
    /// The last character, with its offset and whether it is complex.
    last: Option<(usize, char, bool)>,
}

/// The switches that put back ICU4X's ways with complex runs: the runs of
/// characters its word segmenter hands to a dictionary.
pub(super) struct Legacy;

impl Legacy {
    /// The model of ICU4X's complex runs decides inside them, in place of
    /// the crate's runs. With no other switch on it gives the crate's
    /// boundaries.
    pub(super) const RUN_MODEL: u32 = 1 << 0;
    /// A boundary falls before a complex character after any other, even a
    /// mark or a letter WB5 would join.
    pub(super) const RUN_STARTS: u32 = 1 << 1;
    /// A boundary falls after a complex character before any other, even a
    /// mark, a format character or a joiner (WB4).
    pub(super) const RUN_ENDS: u32 = 1 << 2;
    /// ICU4X's table of script ranges parts a run, rather than the scripts:
    /// its Lao leaves out U+0EA3 and its Chinese and Japanese has holes. A
    /// boundary falls at the end of every part its words reach, and a part
    /// with no dictionary is one word.
    pub(super) const PIECES: u32 = 1 << 3;
    /// A word with no longer one after it ends where the dictionary says,
    /// even inside a cluster.
    pub(super) const LEAVES: u32 = 1 << 4;
    /// Where no word is found, everything read is one word.
    pub(super) const MISSES: u32 = 1 << 5;
    /// At a part's end, after a word and the start of another that ends no
    /// word, the rest is one word.
    pub(super) const REST: u32 = 1 << 6;
    /// A complex character reads as a class of its own that no rule joins,
    /// to the rules and to their lookahead.
    pub(super) const COMPLEX_CLASS: u32 = 1 << 8;
    /// After a Katakana and a zero width joiner, an ExtendNumLet does not
    /// join (WB13a): a hole in ICU4X's state table, which joins it after a
    /// Katakana alone, or a Katakana and an Extend.
    pub(super) const KATAKANA_JOINER: u32 = 1 << 9;
    /// Han and Hiragana are complex without a dictionary too, and a run of
    /// them is then one word.
    pub(super) const CJ_WHOLE: u32 = 1 << 7;
    /// Katakana is not complex: the rules keep a run of it whole (WB13),
    /// where ICU4C's word rules and the crate hand it to the Chinese and
    /// Japanese dictionary.
    pub(super) const KATAKANA_RULES: u32 = 1 << 10;
    /// All of ICU4X's ways where every script has a dictionary.
    pub(super) const ICU: u32 = Self::RUN_MODEL
        | Self::RUN_STARTS
        | Self::RUN_ENDS
        | Self::COMPLEX_CLASS
        | Self::KATAKANA_JOINER
        | Self::KATAKANA_RULES
        | Self::PIECES
        | Self::LEAVES
        | Self::MISSES
        | Self::REST;
}

/// Returns ICU4X's complex script of `ch`, as its `get_complex_script`
/// reads it; a complex character outside its ranges is in a part with no
/// dictionary.
fn icu_complex_script(ch: char) -> RunScript {
    match u32::from(ch) {
        0xE01..=0xE3A | 0xE40..=0x0E4E => RunScript::Thai,
        0x0E81
        | 0x0E82
        | 0x0E84
        | 0x0E86..=0x0E8A
        | 0x0E8C..0x0EA3
        | 0x0EA5
        | 0x0EA7..=0x0EBD
        | 0x0EC0..=0x0EC4
        | 0x0EC6
        | 0x0EC8..=0x0ECE
        | 0x0EDC..=0x0EDF => RunScript::Lao,
        0x1000..=0x103F
        | 0x1050..=0x108F
        | 0x109A..=0x109F
        | 0xA9E0..=0xA9EF
        | 0xA9FA..=0xA9FE
        | 0xAA60..=0xAA7F => RunScript::Myanmar,
        0x1780..=0x17d3 | 0x17d7 | 0x17dc | 0x17dd => RunScript::Khmer,
        0x2E80..=0x2E99
        | 0x2E9B..=0x2EF3
        | 0x2F00..=0x2FD5
        | 0x3005
        | 0x3007
        | 0x3021..=0x3029
        | 0x3038..=0x303B
        | 0x3041..=0x3096
        | 0x309D..=0x309F
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFA6D
        | 0xFA70..=0xFAD9
        | 0x16FE2
        | 0x16FE3
        | 0x16FF0..=0x16FF6
        | 0x1B001..=0x1B11F
        | 0x1B132
        | 0x1B150..=0x1B152
        | 0x1F200
        | 0x20000..=0x2A6DF
        | 0x2A700..=0x2B81D
        | 0x2B820..=0x2CEAD
        | 0x2CEB0..=0x2EBE0
        | 0x2EBF0..=0x2EE5D
        | 0x2F800..=0x2FA1D
        | 0x30000..=0x3134A
        | 0x31350..=0x33479 => RunScript::ChineseJapanese,
        _ => RunScript::Other,
    }
}

impl<I: Iterator<Item = (usize, char)> + Clone> WordBreaks<'_, I> {
    /// Returns the rules' answer before a character of class `class`, with
    /// ICU4X's state table hole put back where switched on.
    pub(super) fn icu_answer(&self, answer: u8, class: u8) -> u8 {
        if self.test.legacy & Legacy::KATAKANA_JOINER != 0
            && self.after_zwj
            && self.state == rules::KATAKANA
            && class == rules::EXTEND_NUM_LETTER
        {
            return rules::BREAK;
        }
        answer
    }

    /// Returns whether the model hands `ch` to a dictionary.
    fn is_complex(&self, ch: char) -> bool {
        self.is_complex_props(self.props(ch))
    }

    /// Returns whether the model hands a character whose word byte is
    /// `props` to a dictionary.
    pub(super) fn is_complex_props(&self, props: WordProps) -> bool {
        match props.run() {
            RUN_SOUTHEAST_ASIAN => true,
            RUN_CHINESE_JAPANESE => {
                let legacy = self.test.legacy;
                (self.cj_runs || legacy & Legacy::CJ_WHOLE != 0)
                    && !(legacy & Legacy::KATAKANA_RULES != 0 && props.class() == rules::KATAKANA)
            }
            _ => false,
        }
    }

    /// Returns the part of a run `ch` is in, under the switches.
    fn model_script(&self, ch: char) -> RunScript {
        if self.test.legacy & Legacy::PIECES != 0 {
            icu_complex_script(ch)
        } else if self.props(ch).run() == RUN_SOUTHEAST_ASIAN {
            RunScript::from_sa_char(ch)
        } else {
            RunScript::ChineseJapanese
        }
    }

    /// Returns whether a boundary falls before `ch`, at `offset`, where the
    /// model of ICU4X's complex runs decides, or else `run`, the crate's
    /// answer.
    ///
    /// As ICU4X's word segmenter does, the model segments a complex run from
    /// its first character, and the boundaries found answer until they are
    /// used up; if they leave the run's end uncovered, it segments again from
    /// the character before.
    pub(super) fn model_step(
        &mut self,
        offset: usize,
        ch: char,
        run: Option<bool>,
    ) -> Option<bool> {
        let legacy = self.test.legacy;
        if legacy & Legacy::RUN_MODEL == 0 {
            return run;
        }
        let complex = self.is_complex(ch);
        let previous = self.test.last.replace((offset, ch, complex));
        let was_complex = previous.is_some_and(|(_, _, complex)| complex);
        if !complex {
            self.test.model = None;
            return (was_complex && legacy & Legacy::RUN_ENDS != 0).then_some(true);
        }
        if !was_complex {
            self.segment_model(offset, &[(offset, ch)]);
            return (legacy & Legacy::RUN_STARTS != 0).then_some(true);
        }
        let (start, first, _) = previous?;
        // Where the script changes between two with no dictionary, the
        // rules decide, as in the crate's runs.
        let script = self.model_script(ch);
        let seam = self.model_script(first) != script
            && script.find(self.dictionaries).is_none()
            && self.model_script(first).find(self.dictionaries).is_none();
        let found = self.model_answer(offset, start, first, ch);
        if seam && found == Some(false) {
            return None;
        }
        found
    }

    /// Returns whether the model's boundaries hold one at `offset`, inside a
    /// complex run after `first`, at `start`; segments again from `first`
    /// where they are used up short of the run's end.
    fn model_answer(&mut self, offset: usize, start: usize, first: char, ch: char) -> Option<bool> {
        if let Some((breaks, end)) = &mut self.test.model {
            let found = breaks.first() == Some(&offset);
            breaks.retain(|&at| at > offset);
            let covered = end.is_some_and(|end| offset < end);
            if breaks.is_empty() && !covered {
                self.test.model = None;
            }
            if found {
                return Some(true);
            }
            if self.test.model.is_some() {
                return Some(false);
            }
        }
        self.segment_model(offset, &[(start, first), (offset, ch)])
    }

    /// Segments the complex run that starts with `head` and goes on after
    /// `offset`, keeps the boundaries after `offset`, and returns whether one
    /// falls at it.
    fn segment_model(&mut self, offset: usize, head: &[(usize, char)]) -> Option<bool> {
        let mut run: Vec<(usize, char)> = head.to_vec();
        let mut least = offset.saturating_add(1);
        for (at, next) in self.scalars.clone() {
            if at < least || !self.is_complex(next) {
                break;
            }
            run.push((at, next));
            least = at.saturating_add(1);
        }
        let legacy = self.test.legacy;
        let quirks = RunQuirks {
            slice_ends: legacy & Legacy::PIECES != 0,
            leaves: legacy & Legacy::LEAVES != 0,
            misses: legacy & Legacy::MISSES != 0,
            rest: legacy & Legacy::REST != 0,
        };
        let (mut breaks, covers) = icu_run_breaks(
            &run,
            self.dictionaries,
            |ch| self.model_script(ch),
            quirks,
            |ch| self.grapheme_props(ch),
            self.grapheme_start(),
            |ch| rules::is_attached(self.class(self.props(ch))),
        );
        let found = breaks.first() == Some(&offset);
        breaks.retain(|&at| at > offset);
        let end = run
            .last()
            .map(|&(at, ch)| at + ch.len_utf8())
            .filter(|_| covers);
        self.test.model = (!breaks.is_empty() || end.is_some()).then_some((breaks, end));
        Some(found)
    }
}
