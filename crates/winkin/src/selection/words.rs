//! Word starts and ends for word motion, found in place at each caret stop.
//!
//! - Words are UAX #29's over each whole paragraph, so a wrap does not split
//!   them, as Blink's `TextOffsetMapping` reads the block whole. Blink's
//!   one tailoring holds: ICU's `en_US_POSIX` rules join a full stop only
//!   between digits, so `x.y` is three words and `3.14` one.
//! - An atomic inline's U+FFFC is a word of its own, as Blink reads a
//!   replaced element as a comma.
//! - The layout's dictionaries segment Thai, Lao, Khmer, Myanmar, Chinese
//!   and Japanese, as ICU4C's do. Without them, a run of the first four is
//!   one word, and each ideograph or hiragana is a word, a katakana run one
//!   word.
//! - To motion, a word is any segment but white space. Punctuation stops a
//!   word motion, and a forced break is a stop of its own. A run of
//!   punctuation and symbols is one word.
//!
//! A motion asks at each stop whether a word starts or ends there
//! ([`Words::starts_word`], [`Words::ends_word`]). Only the clusters around
//! the stop are segmented, [`CONTEXT`] either side: UAX #29 reads at most
//! two characters either side (WB6, WB7, WB11, WB12), past attached marks
//! and format characters (WB4). Regional indicators pair from a cluster
//! start (WB15, WB16),
//! as clusters pair them (GB12, GB13). A motion therefore costs the
//! distance it moves and keeps nothing between calls. Between ASCII
//! letters, digits and spaces, no segmenter runs.
//!
//! A run a dictionary segments ([`Dictionaries::segments`]) is scanned from
//! its start, since a word end inside one depends on all of the run before
//! it. [`Words`] keeps the words either side of the last such stop, which
//! answer later stops between them.
//!
//! Blink's word iterator ignores a builder break opportunity (a `<wbr>`'s
//! U+200B), so `super<wbr>califragilistic` is one word. The scans read the
//! clusters without them, except between regional indicators, which they
//! keep apart. Nothing is copied and nothing allocates.

use core::ops::Range;
use core::str::CharIndices;

use cleft::{Dictionaries, WordBreaks, WordOptions};

use crate::data::{Id, IdRange, TextOffset};
use crate::layout::Layout;
use crate::stages::analysis::{ClusterClass, ClusterId, Clusters};
use crate::{unicode, work};

/// The clusters either side of a stop that a scan reads.
///
/// Format characters standing alone do not count. One more than the two
/// characters UAX #29 reads, to cover a cluster of marks alone.
const CONTEXT: usize = 3;

/// The most clusters a scan reaches over either side, counted or not, so
/// that a long run of format characters costs no more.
const REACH: usize = 64;

/// The options words are found with: ICU's `en_US_POSIX` full stop, as
/// Blink's word iterator has it.
const OPTIONS: WordOptions = WordOptions {
    posix_full_stop: true,
};

/// The dictionaries of a layout built before any had been set.
static NO_DICTIONARIES: Dictionaries = Dictionaries::new();

/// A side of a boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Side {
    /// The text before it.
    Before,
    /// The text after it.
    After,
}

/// Word boundaries in a layout, asked stop by stop by one motion.
///
/// Keeps the words either side of the last stop inside a run a dictionary
/// segments, which answer later stops between them.
pub(super) struct Words<'l> {
    layout: &'l Layout,
    /// The dictionaries the layout was built with.
    dictionaries: &'l Dictionaries,
    /// The words around the last stop asked about inside a run a
    /// dictionary segments.
    known: Option<Known>,
    /// The clusters of the paragraph last asked about, reused until a motion
    /// leaves it.
    paragraph: Option<Range<ClusterId>>,
}

/// The words of a run a dictionary segments either side of a stop inside
/// it.
#[derive(Clone, Debug)]
struct Known {
    /// The cluster the run starts in.
    run: ClusterId,
    /// Its word boundaries either side of the stop, with none between them,
    /// each at the start of the text after it.
    around: Range<TextOffset>,
}

impl<'l> Words<'l> {
    /// Creates the word boundaries of `layout`, with nothing segmented yet.
    pub(super) fn new(layout: &'l Layout) -> Self {
        Self {
            layout,
            dictionaries: layout
                .dictionaries()
                .map_or(&NO_DICTIONARIES, |dictionaries| &**dictionaries),
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

    /// Returns the clusters of the paragraph holding `cluster`, or `None` in
    /// a cleared analysis.
    ///
    /// Reuses the last paragraph without a search where it holds `cluster`.
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
    /// A paragraph's ends are boundaries. Elsewhere, UAX #29 with the
    /// `en_US_POSIX` full stop decides over the clusters around the stop,
    /// ignoring builder break opportunities. Inside a run a dictionary
    /// segments, the run's words decide.
    fn is_boundary(&mut self, stop: ClusterId) -> bool {
        let layout = self.layout;
        let clusters = &layout.analysis().clusters;
        let text = layout.text();
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
        // A run of punctuation and symbols is one word: `a,,,,b`
        // is three words and a masked password one. No stop parts two of
        // them.
        let punctuation = |cluster| {
            clusters
                .first_char(text.into(), cluster)
                .is_some_and(is_punctuation_or_symbol)
        };
        if punctuation(before) && punctuation(after) {
            return false;
        }
        // Regional indicators either side of a builder break opportunity
        // stay apart, as they are drawn.
        if !adjacent && indicator(clusters, text, before) && indicator(clusters, text, after) {
            return true;
        }
        let at = clusters.start(after);
        // Inside a run a dictionary segments, the run's own words decide.
        let segments = |ch: char| self.dictionaries.segments(ch);
        if clusters
            .last_char(text.into(), before)
            .is_some_and(segments)
            && clusters
                .first_char(text.into(), after)
                .is_some_and(segments)
        {
            return self.inside_run(&paragraph, before, at);
        }
        let first = reach(clusters, text, &paragraph, before, Side::Before);
        let last = reach(clusters, text, &paragraph, after, Side::After);
        self.scan(first..last)
            .find(|&boundary| {
                work::step();
                boundary >= at.get()
            })
            .is_some_and(|boundary| boundary == at.get())
    }

    /// Returns the word boundaries of the text of `range`, a paragraph's
    /// clusters, without builder break opportunities.
    fn scan(&self, range: Range<ClusterId>) -> WordBreaks<'l, ParagraphChars<'l>> {
        let layout = self.layout;
        let chars = ParagraphChars {
            clusters: &layout.analysis().clusters,
            text: layout.text(),
            next: range.start,
            end: range.end,
            chars: "".char_indices(),
            base: 0,
        };
        WordBreaks::new(chars, OPTIONS).with_dictionaries(self.dictionaries)
    }

    /// Returns whether the stop between `before` and the text at `at`,
    /// inside a run a dictionary segments, is a word boundary.
    ///
    /// The kept words answer where they surround the stop. Otherwise scans
    /// the run from its start and keeps the words either side of the stop.
    fn inside_run(
        &mut self,
        paragraph: &Range<ClusterId>,
        before: ClusterId,
        at: TextOffset,
    ) -> bool {
        let clusters = &self.layout.analysis().clusters;
        let run = self.run_start(paragraph, before);
        if let Some(known) = &self.known
            && known.run == run
            && known.around.start <= at
            && at <= known.around.end
        {
            return at == known.around.start || at == known.around.end;
        }
        let mut start = clusters.start(run);
        let mut end = clusters.start(paragraph.end);
        for boundary in self.scan(run..paragraph.end) {
            work::step();
            let boundary = TextOffset::new(boundary);
            if boundary >= at {
                end = boundary;
                break;
            }
            start = boundary;
        }
        self.known = Some(Known {
            run,
            around: start..end,
        });
        at == start || at == end
    }

    /// Returns the cluster that the run a dictionary segments, ending with
    /// `before`, starts in, across builder break opportunities.
    fn run_start(&self, paragraph: &Range<ClusterId>, before: ClusterId) -> ClusterId {
        let clusters = &self.layout.analysis().clusters;
        let text = self.layout.text();
        let segments = |ch: char| self.dictionaries.segments(ch);
        let mut cluster = before;
        loop {
            work::step();
            if !cluster_text(clusters, text, cluster).chars().all(segments) {
                return cluster;
            }
            match text_beside(clusters, paragraph, cluster, Side::Before) {
                Some(previous)
                    if clusters
                        .last_char(text.into(), previous)
                        .is_some_and(segments) =>
                {
                    cluster = previous;
                }
                _ => return cluster,
            }
        }
    }

    /// Returns whether the segment on `side` of the word boundary at
    /// `stop`'s start holds anything but white space.
    ///
    /// Stops reading at the first cluster that is not white space, or at the
    /// next boundary.
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
            if !is_white_space(cluster_text(clusters, text, cluster)) {
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

/// The characters of a paragraph's clusters from one on, with their offsets
/// in the layout's text, without builder break opportunities but those
/// between regional indicators.
#[derive(Clone)]
struct ParagraphChars<'l> {
    clusters: &'l Clusters,
    text: &'l str,
    /// The next cluster to read.
    next: ClusterId,
    /// The paragraph's end.
    end: ClusterId,
    /// The characters left of the cluster being read.
    chars: CharIndices<'l>,
    /// The offset of `chars` in the text.
    base: usize,
}

impl Iterator for ParagraphChars<'_> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<(usize, char)> {
        loop {
            if let Some((at, ch)) = self.chars.next() {
                return Some((self.base + at, ch));
            }
            if self.next >= self.end {
                return None;
            }
            work::step();
            let cluster = self.next;
            self.next = ClusterId::new(cluster.get() + 1);
            if self.clusters.class(cluster) == Some(ClusterClass::BreakOpportunity)
                && !between_indicators(self.clusters, self.text, cluster)
            {
                continue;
            }
            self.base = self.clusters.start(cluster).get();
            self.chars = cluster_text(self.clusters, self.text, cluster).char_indices();
        }
    }
}

/// Returns the text of `cluster`.
fn cluster_text<'t>(clusters: &Clusters, text: &'t str, cluster: ClusterId) -> &'t str {
    let range = clusters.range(cluster);
    text.get(range.start.get()..range.end.get())
        .unwrap_or_default()
}

/// Returns the cluster of text on `side` of the boundary before `at`,
/// within `paragraph`.
///
/// Skips builder break opportunities, which words ignore. Returns `None` at
/// the paragraph's end on that side.
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
/// within `paragraph` and at most [`REACH`] clusters. Before the stop it is
/// the first cluster; after, the cluster past the last.
fn reach(
    clusters: &Clusters,
    text: &str,
    paragraph: &Range<ClusterId>,
    from: ClusterId,
    side: Side,
) -> ClusterId {
    let mut counted = 0;
    let mut at = from;
    for _ in 0..REACH {
        work::step();
        if clusters.class(at) != Some(ClusterClass::BreakOpportunity)
            && !clusters.continues_grapheme(text, at)
            && counts(clusters, text, at)
        {
            counted += 1;
        }
        let done = counted >= CONTEXT;
        match side {
            // Back to a grapheme's start, which a split grapheme's later
            // part is not.
            Side::Before => {
                if (done && !clusters.continues_grapheme(text, at)) || at <= paragraph.start {
                    return at;
                }
                at = ClusterId::new(at.get() - 1);
            }
            // On to a grapheme's end, which is not where the next cluster
            // continues a split grapheme.
            Side::After => {
                let next = ClusterId::new(at.get() + 1);
                if (done && !clusters.continues_grapheme(text, next)) || next >= paragraph.end {
                    return next;
                }
                at = next;
            }
        }
    }
    match side {
        Side::Before => at,
        Side::After => ClusterId::new(at.get() + 1).min(paragraph.end),
    }
}

/// Returns whether `cluster` counts among the clusters around a stop.
///
/// It counts unless it starts with a default-ignorable character, which
/// UAX #29 attaches to the text before it (WB4).
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

/// Returns whether the builder break opportunity `cluster` has regional
/// indicators on both sides, which it keeps apart.
fn between_indicators(clusters: &Clusters, text: &str, cluster: ClusterId) -> bool {
    let before = cluster.get().checked_sub(1).map(ClusterId::new);
    let after = ClusterId::new(cluster.get() + 1);
    before.is_some_and(|before| indicator(clusters, text, before))
        && indicator(clusters, text, after)
}

/// Returns `cluster`'s byte where it is one ASCII character.
fn ascii(clusters: &Clusters, text: &str, cluster: ClusterId) -> Option<u8> {
    match cluster_text(clusters, text, cluster).as_bytes() {
        &[byte] if byte.is_ascii() => Some(byte),
        _ => None,
    }
}

/// Returns whether `ch` is punctuation, a symbol or an emoji, which runs
/// together into one word.
///
/// That is anything but a letter, a digit, an ideograph, white space, a
/// control, a format character, a regional indicator (which pairs into a
/// flag of its own), or an atomic inline's U+FFFC.
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
