//! Word starts and ends for word motion, found in place at each caret stop.
//!
//! - Words are UAX #29's over each whole paragraph, so a wrap does not split
//!   them, as Chrome's `TextOffsetMapping` reads the block whole. Chrome's
//!   one tailoring holds: ICU's `en_US_POSIX` rules join a full stop only
//!   between digits, so `x.y` is three words and `3.14` one.
//! - An atomic inline's U+FFFC is a word of its own, as Blink reads a
//!   replaced element as a comma.
//! - With the `dictionaries` feature, ICU's dictionaries segment Thai, Lao,
//!   Khmer, Myanmar, Chinese and Japanese, as ICU4C does for Chrome. Without
//!   it, the LSTM segments the first four, and each ideograph or hiragana is
//!   a word, a katakana run one word.
//! - To motion, a word is any segment but white space. Punctuation stops a
//!   word motion, and a forced break is a stop of its own. A run of
//!   punctuation and symbols is one word, as Chrome takes it.
//!
//! A motion asks at each stop whether a word starts or ends there
//! ([`Words::starts_word`], [`Words::ends_word`]). Only the clusters around
//! the stop are segmented. UAX #29 reads at most two characters either side
//! (WB6, WB7, WB11, WB12), past attached marks and format characters (WB4),
//! so [`CONTEXT`] clusters either side decide as the whole paragraph would.
//! Regional indicators pair from a cluster start (WB15, WB16), as clusters
//! pair them (GB12, GB13). So a motion costs the distance it moves and keeps
//! nothing between calls. Between ASCII letters, digits and spaces, no
//! segmenter is needed.
//!
//! Runs ICU hands whole to its dictionaries or LSTM (`unicode::is_complex`)
//! are segmented whole, since a word end inside one depends on all of it.
//! [`Words`] keeps the words either side of the last such stop, so a motion
//! segments each run it enters once. These runs are where word motion
//! allocates, inside ICU.
//!
//! A builder break opportunity (a `<wbr>`'s U+200B) is no text to Chrome's
//! iterator, so `super<wbr>califragilistic` is one word. Where one is near a
//! stop, the clusters are copied without it into a stack buffer and
//! segmented there. A cluster longer than [`CLUSTER_BYTES`] is copied as its
//! first and last characters, which is all the rules read. Regional
//! indicators either side of one stay apart. A whole run longer than [`RUN`]
//! bytes is segmented in the pieces its break opportunities part.
//!
//! Without dictionaries, an ideograph or hiragana is copied as a stand-in of
//! its Word_Break (`unicode::segmenter_char`), so it never reaches ICU's
//! dictionary, which the build does not carry.

use core::ops::Range;
use core::str;

use icu_segmenter::WordSegmenterBorrowed;

use crate::data::{Id, IdRange, TextOffset};
use crate::layout::Layout;
use crate::stages::analysis::{ClusterClass, ClusterId, Clusters};
use crate::unicode::{has_stand_ins, is_complex, segmenter_char, word_boundaries, word_segmenter};
use crate::{unicode, work};

/// The clusters either side of a stop that decide a word boundary.
///
/// Format characters standing alone do not count. It is one more than the
/// two characters UAX #29 reads, for a cluster of marks alone.
const CONTEXT: usize = 3;

/// The size of the stack buffer for the clusters around a stop.
///
/// It holds [`CONTEXT`] clusters either side, each at most
/// [`CLUSTER_BYTES`], and a few format characters.
const WINDOW: usize = 128;

/// The most bytes of a cluster copied whole into the buffer.
///
/// A longer one is copied as its first and last characters. UAX #29 reads a
/// cluster's edges by those: marks and joiners attach to the first (WB4),
/// and a joiner before a pictograph is read as the last (WB3c).
const CLUSTER_BYTES: usize = 16;

/// The size of the stack buffer for a run segmented whole.
///
/// It holds over three hundred Thai characters or ideographs. A Thai
/// paragraph measured in Chrome runs at most 52 between spaces.
const RUN: usize = 1024;

/// Which side of a boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Side {
    /// The text before it.
    Before,
    /// The text after it.
    After,
}

/// Word boundaries in a layout, asked stop by stop by one motion.
///
/// It keeps the words either side of the last stop inside a run segmented
/// whole, which answer later stops between them.
pub(super) struct Words<'l> {
    layout: &'l Layout,
    segmenter: WordSegmenterBorrowed<'static>,
    /// The words of the run a stop was last asked about inside.
    known: Option<Known>,
    /// The clusters of the paragraph last asked about, which a motion asks
    /// again at each stop until it leaves it.
    paragraph: Option<Range<ClusterId>>,
}

/// The words of a run segmented whole either side of a stop inside it.
#[derive(Clone, Debug)]
struct Known {
    /// The run's text.
    run: Range<TextOffset>,
    /// Its word boundaries either side of the stop, with none between them.
    ///
    /// Each is after the text before it, before any builder break
    /// opportunity there.
    around: Range<TextOffset>,
}

impl<'l> Words<'l> {
    /// Returns the words of `layout`, with nothing yet segmented.
    pub(super) fn new(layout: &'l Layout) -> Self {
        Self {
            layout,
            segmenter: word_segmenter(),
            known: None,
            paragraph: None,
        }
    }

    /// Returns whether a word starts at the caret stop at `stop`'s start.
    ///
    /// A forward motion that skips spaces stops there, and so does every
    /// backward motion.
    pub(super) fn starts_word(&mut self, stop: ClusterId) -> bool {
        self.is_boundary(stop) && self.word_beside(stop, Side::After)
    }

    /// Returns whether a word ends at the caret stop at `stop`'s start.
    ///
    /// A forward motion that stops at word ends stops there.
    pub(super) fn ends_word(&mut self, stop: ClusterId) -> bool {
        self.is_boundary(stop) && self.word_beside(stop, Side::Before)
    }

    /// Returns the clusters of the paragraph holding `cluster`, or `None` in a cleared analysis.
    ///
    /// The paragraph last asked about answers without a search where it
    /// holds `cluster`.
    fn paragraph(&mut self, cluster: ClusterId) -> Option<Range<ClusterId>> {
        if let Some(paragraph) = &self.paragraph
            && paragraph.contains(&cluster)
        {
            return Some(paragraph.clone());
        }
        let found = self
            .layout
            .analysis()
            .paragraphs
            .clusters_containing(cluster);
        self.paragraph.clone_from(&found);
        found
    }

    /// Returns whether the caret stop at `stop`'s start is a word boundary.
    ///
    /// A paragraph's ends are boundaries. Elsewhere, UAX #29 with Chrome's
    /// full stops decides over the clusters around the stop, ignoring builder
    /// break opportunities. Inside a run segmented whole, the run's words
    /// decide.
    fn is_boundary(&mut self, stop: ClusterId) -> bool {
        let layout = self.layout;
        let analysis = layout.analysis();
        let clusters = &analysis.clusters;
        let text = layout.text();
        let at = clusters.start(stop).get();
        // Each paragraph is segmented on its own, so its ends are boundaries.
        let Some(paragraph) = stop
            .get()
            .checked_sub(1)
            .and_then(|before| self.paragraph(ClusterId::new(before)))
            .filter(|paragraph| stop < paragraph.end)
        else {
            return true;
        };
        let (Some(before), Some(after)) = (
            text_beside(clusters, &paragraph, stop, Side::Before),
            text_beside(clusters, &paragraph, stop, Side::After),
        ) else {
            return true;
        };
        let adjacent = after.get() == before.get() + 1;
        if adjacent
            && let (Some(left), Some(right)) =
                (ascii(clusters, text, before), ascii(clusters, text, after))
        {
            // Two letters or digits never break (WB5, WB8, WB9, WB10). One
            // and a space always break (WB999). Two spaces never break (WB3d).
            let word = |byte: u8| byte.is_ascii_alphanumeric();
            match (left, right) {
                _ if word(left) && word(right) => return false,
                (b' ', b' ') => return false,
                _ if (word(left) && right == b' ') || (left == b' ' && word(right)) => {
                    return true;
                }
                _ => {}
            }
        }
        // Chrome takes a run of punctuation and symbols as one word, as it
        // takes `a,,,,b` (three words) or a masked password (one): no stop
        // parts two of them.
        let punctuation = |cluster| {
            clusters
                .first_char(text.into(), cluster)
                .is_some_and(is_punctuation_or_symbol)
        };
        if punctuation(before) && punctuation(after) {
            return false;
        }
        // Inside a run ICU segments whole, the run's own words decide.
        if clusters
            .last_char(text.into(), before)
            .is_some_and(is_complex)
            && clusters
                .first_char(text.into(), after)
                .is_some_and(is_complex)
        {
            return self.inside_run(&paragraph, before, after);
        }
        // Regional indicators either side of a builder break opportunity
        // stay apart, as they are drawn.
        if !adjacent && indicator(clusters, text, before) && indicator(clusters, text, after) {
            return true;
        }
        let (first, before_generated) = reach(clusters, text, &paragraph, before, Side::Before);
        let (last, after_generated) = reach(clusters, text, &paragraph, after, Side::After);
        let from = clusters.start(first).get();
        let to = clusters.start(last).get();
        if adjacent
            && !before_generated
            && !after_generated
            && let Some(window) = text.get(from..to)
            && !has_stand_ins(window)
        {
            // With no builder break opportunity nearby and no stand-ins,
            // the text is segmented in place.
            return is_word_boundary(self.segmenter, window, at.saturating_sub(from));
        }
        // Otherwise the clusters are copied onto the stack, without builder
        // break opportunities and with stand-ins for ideographs and hiragana.
        let mut buffer = [0u8; WINDOW];
        let mut len = 0;
        let mut stop_at = 0;
        for cluster in (first..last).ids() {
            work::step();
            if clusters.class(cluster) == Some(ClusterClass::BreakOpportunity)
                && !between_indicators(clusters, text, cluster)
            {
                continue;
            }
            let range = clusters.range(cluster);
            let Some(piece) = text.get(range.start.get()..range.end.get()) else {
                continue;
            };
            let Some(copied) = copy_short(piece, buffer.get_mut(len..).unwrap_or_default()) else {
                break;
            };
            len += copied;
            if cluster == before {
                stop_at = len;
            }
        }
        buffer
            .get(..len)
            .and_then(|bytes| str::from_utf8(bytes).ok())
            .is_none_or(|window| is_word_boundary(self.segmenter, window, stop_at))
    }

    /// Returns whether the stop between `before` and `after`, inside a run ICU segments whole, is a word boundary.
    ///
    /// The kept words answer where they surround the stop. Otherwise the run
    /// is segmented whole, and the words either side of the stop are kept.
    fn inside_run(
        &mut self,
        paragraph: &Range<ClusterId>,
        before: ClusterId,
        after: ClusterId,
    ) -> bool {
        let layout = self.layout;
        let clusters = &layout.analysis().clusters;
        let text = layout.text();
        // The stop as the run has it: after the text before it, on either
        // side of a builder break opportunity.
        let at = clusters.range(before).end;
        if let Some(known) = &self.known
            && known.run.start < at
            && at < known.run.end
            && known.around.start <= at
            && at <= known.around.end
        {
            return at == known.around.start || at == known.around.end;
        }
        let mut found = complex_run(clusters, text, paragraph, before, after, true);
        if found.2.is_some_and(|copied| copied > RUN) {
            // Too long to copy, so it is segmented in the pieces its builder
            // break opportunities part, which meet at a boundary.
            if after.get() != before.get() + 1 {
                return true;
            }
            found = complex_run(clusters, text, paragraph, before, after, false);
        }
        let (run, run_clusters, copied) = found;
        let around = if copied.is_some() {
            // Copy it onto the stack without builder break opportunities,
            // then map the words found back into the text.
            let mut buffer = [0u8; RUN];
            let mut len = 0;
            let mut stop_at = None;
            each_piece(clusters, text, &run, &run_clusters, |start, piece| {
                if stop_at.is_none() && start <= at && at.get() <= start.get() + piece.len() {
                    stop_at = Some(len + (at.get() - start.get()));
                }
                if let Some(into) = buffer.get_mut(len..len + piece.len()) {
                    into.copy_from_slice(piece.as_bytes());
                    len += piece.len();
                }
            });
            let copy = buffer
                .get(..len)
                .and_then(|bytes| str::from_utf8(bytes).ok());
            let (Some(copy), Some(stop_at)) = (copy, stop_at) else {
                return true;
            };
            let (from, to) = words_around(self.segmenter, copy, stop_at);
            let put = |at: usize| put_back(clusters, text, &run, &run_clusters, at);
            put(from)..put(to)
        } else {
            let Some(copy) = text.get(run.start.get()..run.end.get()) else {
                return true;
            };
            let (from, to) = words_around(
                self.segmenter,
                copy,
                at.get().saturating_sub(run.start.get()),
            );
            TextOffset::new(run.start.get() + from)..TextOffset::new(run.start.get() + to)
        };
        let boundary = at == around.start || at == around.end;
        self.known = Some(Known { run, around });
        boundary
    }

    /// Returns whether the segment on `side` of the word boundary at `stop`'s start holds anything but white space.
    ///
    /// A segment is known to be a word at its first cluster that is not white
    /// space. White space is read to the next boundary.
    fn word_beside(&mut self, stop: ClusterId, side: Side) -> bool {
        let layout = self.layout;
        let clusters = &layout.analysis().clusters;
        let text = layout.text();
        let holding = match side {
            Side::After => Some(stop),
            Side::Before => stop.get().checked_sub(1).map(ClusterId::new),
        };
        let Some(paragraph) = holding.and_then(|cluster| self.paragraph(cluster)) else {
            return false;
        };
        let Some(mut cluster) = text_beside(clusters, &paragraph, stop, side) else {
            return false;
        };
        loop {
            work::step();
            let range = clusters.range(cluster);
            let piece = text
                .get(range.start.get()..range.end.get())
                .unwrap_or_default();
            if !is_white_space(piece) {
                return true;
            }
            // On to the next cluster of text that way, unless a boundary
            // parts them.
            let edge = match side {
                Side::After => ClusterId::new(cluster.get() + 1),
                Side::Before => cluster,
            };
            let Some(next) = text_beside(clusters, &paragraph, edge, side) else {
                return false;
            };
            if self.is_boundary(edge) {
                return false;
            }
            cluster = next;
        }
    }
}

/// Returns whether UAX #29, with Chrome's full stops, breaks `window` at byte `at`.
///
/// It uses [`word_boundaries`], which adds the breaks
/// `en_US_POSIX` makes around a full stop.
fn is_word_boundary(segmenter: WordSegmenterBorrowed<'static>, window: &str, at: usize) -> bool {
    for boundary in word_boundaries(segmenter, window, 0) {
        work::step();
        if boundary >= at {
            return boundary == at;
        }
    }
    true
}

/// Returns the word boundaries of `run` around byte `at`: the last at or before, the first at or after.
///
/// `run` holds characters ICU segments whole. No full stop joins such
/// characters, so Chrome's full-stop tailoring does not apply.
fn words_around(segmenter: WordSegmenterBorrowed<'static>, run: &str, at: usize) -> (usize, usize) {
    let mut before = 0;
    for end in segmenter.segment_str(run) {
        work::step();
        if end >= at {
            return (if end == at { at } else { before }, end);
        }
        before = end;
    }
    (before, run.len())
}

/// Returns the run of characters ICU segments whole around the stop between `before` and `after`.
///
/// The run stays within `paragraph`. Where `through`, it crosses builder
/// break opportunities, which are no text to words. With the run come the
/// clusters holding it, from the one holding its start to the one holding
/// its end, and its byte length without them, or `None` where it holds none.
fn complex_run(
    clusters: &Clusters,
    text: &str,
    paragraph: &Range<ClusterId>,
    before: ClusterId,
    after: ClusterId,
    through: bool,
) -> (Range<TextOffset>, Range<ClusterId>, Option<usize>) {
    let mut generated = after.get() != before.get() + 1;
    // Back from the stop, a cluster of text at a time, while the run goes on.
    let mut cluster = before;
    let start = loop {
        work::step();
        let range = clusters.range(cluster);
        let piece = text
            .get(range.start.get()..range.end.get())
            .unwrap_or_default();
        if let Some((at, ch)) = piece.char_indices().rev().find(|&(_, ch)| !is_complex(ch)) {
            let start = range.start.get() + at + ch.len_utf8();
            // Past the cluster's last character, the run starts with the next.
            let holding = if start == range.end.get() {
                ClusterId::new(cluster.get() + 1)
            } else {
                cluster
            };
            break (TextOffset::new(start), holding);
        }
        match text_beside(clusters, paragraph, cluster, Side::Before) {
            Some(previous)
                if clusters
                    .last_char(text.into(), previous)
                    .is_some_and(is_complex)
                    && (through || previous.get() + 1 == cluster.get()) =>
            {
                generated |= previous.get() + 1 != cluster.get();
                cluster = previous;
            }
            _ => break (range.start, cluster),
        }
    };
    // Forward from the stop the same way.
    let mut cluster = after;
    let end = loop {
        work::step();
        let range = clusters.range(cluster);
        let piece = text
            .get(range.start.get()..range.end.get())
            .unwrap_or_default();
        if let Some((at, _)) = piece.char_indices().find(|&(_, ch)| !is_complex(ch)) {
            break (TextOffset::new(range.start.get() + at), cluster);
        }
        let next = ClusterId::new(cluster.get() + 1);
        match text_beside(clusters, paragraph, next, Side::After) {
            Some(following)
                if clusters
                    .first_char(text.into(), following)
                    .is_some_and(is_complex)
                    && (through || following == next) =>
            {
                generated |= following != next;
                cluster = following;
            }
            _ => break (range.end, ClusterId::new(cluster.get() + 1)),
        }
    };
    let (run, holding) = (start.0..end.0, start.1..end.1);
    let copied = generated.then(|| {
        let mut len = 0;
        each_piece(clusters, text, &run, &holding, |_, piece| {
            len += piece.len()
        });
        len
    });
    (run, holding, copied)
}

/// Calls `each` with each part of `run`'s text between its builder break opportunities, and its start.
///
/// `holding` is from the cluster holding the run's start to the one holding
/// its end.
fn each_piece(
    clusters: &Clusters,
    text: &str,
    run: &Range<TextOffset>,
    holding: &Range<ClusterId>,
    mut each: impl FnMut(TextOffset, &str),
) {
    let mut from = run.start;
    for cluster in holding.clone().ids() {
        work::step();
        if clusters.class(cluster) != Some(ClusterClass::BreakOpportunity) {
            continue;
        }
        let range = clusters.range(cluster);
        if let Some(piece) = text.get(from.get()..range.start.get()) {
            each(from, piece);
        }
        from = range.end;
    }
    if let Some(piece) = text.get(from.get()..run.end.get()) {
        each(from, piece);
    }
}

/// Returns where byte `at` of `run`'s copy without builder break opportunities is in the text.
///
/// It is after the text before it, before any break opportunity there.
/// `holding` is the clusters holding the run, as [`each_piece`] takes them.
fn put_back(
    clusters: &Clusters,
    text: &str,
    run: &Range<TextOffset>,
    holding: &Range<ClusterId>,
    at: usize,
) -> TextOffset {
    let mut found = None;
    let mut len = 0;
    each_piece(clusters, text, run, holding, |start, piece| {
        if found.is_none() && at <= len + piece.len() {
            found = Some(TextOffset::new(start.get() + at.saturating_sub(len)));
        }
        len += piece.len();
    });
    found.unwrap_or(run.end)
}

/// Returns the cluster of text on `side` of the boundary before `at`, within `paragraph`.
///
/// It skips builder break opportunities, which are no text to words. `None`
/// at the paragraph's end that way.
fn text_beside(
    clusters: &Clusters,
    paragraph: &Range<ClusterId>,
    at: ClusterId,
    side: Side,
) -> Option<ClusterId> {
    let text = |cluster: &ClusterId| {
        work::step();
        clusters.class(*cluster) != Some(ClusterClass::BreakOpportunity)
    };
    match side {
        Side::Before => (paragraph.start..at.min(paragraph.end))
            .ids()
            .rev()
            .find(text),
        Side::After => (at.max(paragraph.start)..paragraph.end).ids().find(text),
    }
}

/// Returns how far the clusters around a stop reach on `side`, from `from`.
///
/// The reach covers [`CONTEXT`] counted clusters, to a grapheme boundary,
/// within `paragraph` and half the buffer. Before, it is the first cluster;
/// after, the cluster past the last. It also says whether a builder break
/// opportunity is among them.
fn reach(
    clusters: &Clusters,
    text: &str,
    paragraph: &Range<ClusterId>,
    from: ClusterId,
    side: Side,
) -> (ClusterId, bool) {
    // Each side fits half the buffer, less the most one cluster takes.
    const BUDGET: usize = WINDOW / 2 - CLUSTER_BYTES;
    let mut counted = 0;
    let mut bytes = 0;
    let mut generated = false;
    let mut at = from;
    loop {
        work::step();
        let generates = clusters.class(at) == Some(ClusterClass::BreakOpportunity);
        generated |= generates;
        if !generates && !clusters.is_continuation(at) && counts(clusters, text, at) {
            counted += 1;
        }
        bytes += short_len(clusters, text, at);
        let done = counted >= CONTEXT || bytes >= BUDGET;
        match side {
            // Back to a grapheme's start, which a split grapheme's later
            // part is not.
            Side::Before => {
                if (done && !clusters.is_continuation(at)) || at <= paragraph.start {
                    return (at, generated);
                }
                at = ClusterId::new(at.get() - 1);
            }
            // On to a grapheme's end, which is not where the next cluster
            // continues a split grapheme.
            Side::After => {
                let next = ClusterId::new(at.get() + 1);
                if (done && !clusters.is_continuation(next)) || next >= paragraph.end {
                    return (next, generated);
                }
                at = next;
            }
        }
    }
}

/// Returns whether `cluster` counts among the clusters around a stop.
///
/// It counts unless it starts with a default-ignorable character, which
/// UAX #29 attaches to what is before it (WB4).
fn counts(clusters: &Clusters, text: &str, cluster: ClusterId) -> bool {
    clusters
        .first_char(text.into(), cluster)
        .is_some_and(|ch| !unicode::rare_props(ch).is_default_ignorable())
}

/// Returns whether `cluster` starts with a regional indicator.
fn indicator(clusters: &Clusters, text: &str, cluster: ClusterId) -> bool {
    clusters
        .first_char(text.into(), cluster)
        .is_some_and(|ch| unicode::core_props(ch).is_regional_indicator())
}

/// Returns whether builder break opportunity `cluster` has regional indicators on both sides.
///
/// It keeps them apart.
fn between_indicators(clusters: &Clusters, text: &str, cluster: ClusterId) -> bool {
    let before = cluster.get().checked_sub(1).map(ClusterId::new);
    let after = ClusterId::new(cluster.get() + 1);
    before.is_some_and(|before| indicator(clusters, text, before))
        && indicator(clusters, text, after)
}

/// Returns the byte `cluster` is, where it is one ASCII character.
fn ascii(clusters: &Clusters, text: &str, cluster: ClusterId) -> Option<u8> {
    let range = clusters.range(cluster);
    match text.as_bytes().get(range.start.get()..range.end.get())? {
        &[byte] if byte.is_ascii() => Some(byte),
        _ => None,
    }
}

/// Returns how many bytes of `cluster` [`copy_short`] copies into the buffer.
fn short_len(clusters: &Clusters, text: &str, cluster: ClusterId) -> usize {
    let range = clusters.range(cluster);
    let piece = text
        .get(range.start.get()..range.end.get())
        .unwrap_or_default();
    shortened(piece).map_or(0, |(first, last)| first.len() + last.len())
}

/// Returns `piece` as it is copied: whole, or its first and last characters where longer than [`CLUSTER_BYTES`].
fn shortened(piece: &str) -> Option<(&str, &str)> {
    if piece.len() <= CLUSTER_BYTES {
        return Some((piece, ""));
    }
    let first = piece.chars().next()?.len_utf8();
    let last = piece.chars().next_back()?.len_utf8();
    Some((piece.get(..first)?, piece.get(piece.len() - last..)?))
}

/// Copies `piece`, shortened, into `into`, and returns the bytes it took.
///
/// Each character is copied as ICU's word segmenter is given it
/// (`unicode::segmenter_char`), which is the same length. `None` where it does
/// not fit.
fn copy_short(piece: &str, into: &mut [u8]) -> Option<usize> {
    let (first, last) = shortened(piece)?;
    let mut len = 0;
    for ch in first.chars().chain(last.chars()) {
        let ch = segmenter_char(ch).unwrap_or(ch);
        let to = into.get_mut(len..len + ch.len_utf8())?;
        ch.encode_utf8(to);
        len += ch.len_utf8();
    }
    Some(len)
}

/// Returns whether `ch` is punctuation, a symbol or an emoji, which runs
/// together into one word.
///
/// That is anything but a letter, a digit, an ideograph, white space, a
/// control, a format character, a regional indicator, which pairs into a
/// flag of its own, or an atomic inline's U+FFFC.
fn is_punctuation_or_symbol(ch: char) -> bool {
    !(ch.is_alphanumeric()
        || ch.is_whitespace()
        || ch.is_control()
        || ch == '\u{FFFC}'
        || unicode::core_props(ch).is_regional_indicator()
        || unicode::rare_props(ch).is_default_ignorable())
}

/// Returns whether `piece` is white space, which word motion skips.
///
/// White space is spaces, tabs and the other space separators only, so a
/// forced break is a stop of its own.
fn is_white_space(piece: &str) -> bool {
    piece.chars().all(|ch| {
        matches!(
            ch,
            ' ' | '\t' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
        )
    })
}
