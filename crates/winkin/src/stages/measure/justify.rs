//! Justification opportunities: where a justified line puts its extra room,
//! cluster by cluster.
//!
//! The scan, line layout and the readers all ask this one rule, so what a
//! reader walks adds up to what line layout placed. Nothing is stored per
//! cluster.
//!
//! Chrome's rule under `text-justify: auto`
//! (`Character::ExpansionOpportunityCount`,
//! `ShapeResultSpacing::ComputeSpacing`), over the line less what hangs:
//! - a space, a tab or U+00A0 (`TreatAsSpace`) has an opportunity after it;
//! - an ideograph or CJK symbol (`IsCJKIdeographOrSymbol`) has one after
//!   it, and one before it where what precedes it is neither;
//! - the line's last opportunity, after its last character, is dropped;
//! - a zero-width space, control, soft hyphen or lone joiner takes none,
//!   and its neighbours don't see it.
//!
//! `inter-word` keeps the spaces alone. `inter-character` puts one after
//! every cluster but the last, as Chrome's `distribute` does, and one
//! before the first character after a run of atomic inlines. `none`
//! justifies nothing. Each cluster follows its own text's `text-justify`.
//! Hangul is no ideograph, as in Chrome.
//!
//! Combined text (`text-combine-upright`) is one ideograph, as Blink
//! justifies it as U+3042 (`kTextCombineItemMarker`). A ruby base at least
//! as wide as its annotations is justified as the line's text. A narrower
//! one makes its column one object (`kBaseShorterRubyMarker`). Annotation
//! text is unseen.

use core::cell::{Cell, RefCell};
use core::fmt;
use core::ops::Range;

use parlance::Script;

use crate::data::Id;
use crate::data::IdRange;
use crate::stages::LineStages;
use crate::stages::analysis::{
    Analysis, ClusterClass, ClusterId, ParagraphFlags, RunOrientation, ScriptRunId, ScriptRuns,
};
use crate::stages::content::ItemId;
use crate::stages::content::{Content, ContentFlags, NodeId};
use crate::style::{FirstLineVariant, TextJustify};
use crate::work;

use super::ruby_columns::ColumnScope;
use super::{RubyColumn, RubyColumnId, RubyColumns};

/// How a cluster takes part in justification.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    /// A space, a tab or U+00A0: an opportunity after it.
    Space,
    /// An ideograph or a CJK symbol: one after it, and one before it after
    /// [`Other`](Self::Other), [`Object`](Self::Object) or
    /// [`Cursive`](Self::Cursive).
    Cjk,
    /// What Chrome treats as a zero-width space: nothing, and unseen.
    Invisible,
    /// An atomic inline's U+FFFC: nothing, even between characters.
    Object,
    /// A letter of a script whose letters join. Under `inter-character` it
    /// takes nothing, as an atomic inline; otherwise it is [`Other`](Self::Other).
    Cursive,
    /// The first cluster of a combined unit: an ideograph's room before it.
    UnitStart,
    /// The last cluster of a combined unit: an ideograph's room after it.
    UnitEnd,
    /// Anything else.
    Other,
}

impl Kind {
    /// Returns how a cluster of `class` takes part, or `None` where its
    /// character decides.
    #[inline]
    fn from_class(class: ClusterClass) -> Option<Self> {
        match class {
            ClusterClass::Space | ClusterClass::Tab => Some(Self::Space),
            ClusterClass::Control
            | ClusterClass::ZeroWidthSpace
            | ClusterClass::BreakOpportunity
            | ClusterClass::SoftHyphen => Some(Self::Invisible),
            ClusterClass::Object => Some(Self::Object),
            ClusterClass::Separator | ClusterClass::DrawnSeparator => Some(Self::Other),
            ClusterClass::NoBreakSpace
            | ClusterClass::Text
            | ClusterClass::Emoji
            | ClusterClass::Symbol
            | ClusterClass::OtherSpace => None,
        }
    }
}

/// The line-wide facts of a line's opportunities, which line layout keeps
/// so a reader skips a walk over the line.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct JustifySummary {
    ascii: bool,
    spaces: usize,
    last: ClusterId,
}

/// The opportunities of one line: which of its clusters take a share of its
/// extra room, and where.
///
/// They are worked out as they are read. On an ASCII line the class
/// decides; on one also justified at its spaces
/// ([`is_simple`](Self::is_simple)), they are counted from the bytes, a
/// word at a time.
#[derive(Clone)]
pub(crate) struct JustifyOpportunities<'a> {
    content: &'a Content,
    /// The clusters, and the items and script runs where a rule reads them.
    analysis: &'a Analysis,
    justify: TextJustify,
    /// The line's first cluster.
    start: ClusterId,
    /// One past its last cluster that does not hang.
    end: ClusterId,
    /// The line's text is ASCII: no cluster of it is CJK or U+00A0.
    ascii: bool,
    /// Where it is ASCII, how many of its clusters are spaces or tabs.
    spaces: usize,
    /// See [`is_simple`](Self::is_simple).
    simple: bool,
    /// The line's last cluster that its neighbours see, whose opportunity
    /// after is dropped. `end` where there is none.
    last: ClusterId,
    /// The ruby column whose base is walked, where one is. Its child
    /// columns count as one character each.
    scope: Option<RubyColumnId>,
    /// Each cluster's own `text-justify`, where some style sets one other
    /// than `auto`.
    per_cluster: Option<ClusterJustify>,
    /// Some cluster may be justified under `inter-character`.
    characters: bool,
    /// The script runs are read: some text is combined or, under
    /// `inter-character`, some letter may be cursive.
    scripts: bool,
    /// Some text is combined. Each combined unit is one character.
    combined: bool,
    /// The script run and the ruby column the last cluster was read in.
    /// Reads step from here, since a line is read nearly in order.
    near: Cell<ScriptRunId>,
    columns: Option<RefCell<ColumnScope<'a>>>,
}

/// The ruby columns an opportunity walk reads, and the base it walks.
struct RubyScope<'a> {
    columns: ColumnScope<'a>,
    base: Option<RubyColumnId>,
}

/// The `text-justify` of each of a line's clusters, read through an item
/// cursor that steps from the last cluster read.
#[derive(Clone)]
struct ClusterJustify {
    /// The first-line variant the line is set in.
    variant: FirstLineVariant,
    item: Cell<ItemId>,
}

impl ClusterJustify {
    /// Returns the `text-justify` of the text holding `cluster`.
    fn justify(&self, content: &Content, analysis: &Analysis, cluster: ClusterId) -> TextJustify {
        let items = &analysis.item_clusters;
        // The first item at the cluster's start may be a box's edge. The
        // cluster's item is the first from there that holds clusters.
        let item = items.walk_to_cluster(self.item.get(), cluster);
        self.item.set(item);
        content.items.get(item).map_or(TextJustify::Auto, |row| {
            let text = content.nodes.text_facts(row.node, self.variant);
            content.facts.text(text).justify
        })
    }
}

impl fmt::Debug for JustifyOpportunities<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JustifyOpportunities")
            .field("justify", &self.justify)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("ascii", &self.ascii)
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl<'a> JustifyOpportunities<'a> {
    /// Returns the opportunities of `clusters`, a ruby base or annotation,
    /// as a line of their own under `justify` (Chrome's
    /// `ApplyJustification`). `None` under `text-justify: none`.
    pub(crate) fn new(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
    ) -> Option<Self> {
        Self::from_clusters(content, analysis, clusters, justify, None, None, None)
    }

    /// Returns the opportunities of `column`'s base, in which each child
    /// column is one character. The columns are walked to from `near`.
    /// `None` under `text-justify: none`.
    pub(crate) fn from_base(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        rubies: &'a RubyColumns,
        (column, near): (RubyColumnId, RubyColumnId),
        summary: Option<JustifySummary>,
    ) -> Option<Self> {
        let scope = RubyScope {
            columns: ColumnScope::from_column(rubies, near, clusters.start),
            base: Some(column),
        };
        Self::from_clusters(
            content,
            analysis,
            clusters,
            justify,
            Some(scope),
            summary,
            None,
        )
    }

    /// Returns the opportunities of a line set in `stages`' variant, each
    /// cluster under its own text's `text-justify`.
    ///
    /// `clusters` runs from the line's first cluster to its content's end.
    /// `None` where only the block sets `text-justify`, to `none`.
    pub(crate) fn from_line(
        stages: &LineStages<'a>,
        clusters: Range<ClusterId>,
        summary: Option<JustifySummary>,
    ) -> Option<Self> {
        let (content, analysis) = (stages.content, stages.analysis);
        let rubies = stages.measured.ruby_columns();
        // The block's `text-justify`, from its own text facts.
        let block = content
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
        let ruby = content.flags.contains(ContentFlags::RUBY) && !rubies.is_empty();
        let columns = ruby.then(|| ColumnScope::new(rubies, clusters.start));
        let per_cluster =
            content
                .flags
                .contains(ContentFlags::TEXT_JUSTIFY)
                .then(|| ClusterJustify {
                    variant: stages.variant(),
                    item: Cell::new(
                        analysis
                            .item_clusters
                            .cursor_containing(clusters.start)
                            .id(),
                    ),
                });
        // With styles of their own, the clusters decide.
        let justify = match per_cluster {
            Some(_) => TextJustify::Auto,
            None => content.facts.text(block).justify,
        };
        let scope = columns.map(|columns| RubyScope {
            columns,
            base: None,
        });
        Self::from_clusters(
            content,
            analysis,
            clusters,
            justify,
            scope,
            summary,
            per_cluster,
        )
    }

    /// Returns the opportunities of `clusters`, reading the ruby columns
    /// through `scope` and each cluster's `text-justify` through
    /// `per_cluster`. A `summary` saves the walk over the clusters.
    fn from_clusters(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        scope: Option<RubyScope<'a>>,
        summary: Option<JustifySummary>,
        per_cluster: Option<ClusterJustify>,
    ) -> Option<Self> {
        if justify == TextJustify::None && per_cluster.is_none() {
            return None;
        }
        let (start, end) = (clusters.start, clusters.end.max(clusters.start));
        let clusters = &analysis.clusters;
        let text = &content.text;
        let bytes = clusters.start(start).get()..clusters.start(end).get();
        let (ascii, spaces) = summary.map_or_else(
            || {
                text.as_bytes()
                    .get(bytes)
                    .map_or((false, 0), count_ascii_spaces)
            },
            |summary| (summary.ascii, summary.spaces),
        );
        let combined = analysis.flags.contains(ParagraphFlags::HAS_COMBINED);
        // Under `inter-character`, a letter's script may make it cursive.
        let characters = per_cluster.is_some() || justify == TextJustify::InterCharacter;
        let scripts = combined || (characters && !ascii);
        // The one seek: the script run and the column at the line's start.
        let near = scripts
            .then(|| analysis.runs.containing(start))
            .flatten()
            .unwrap_or(ScriptRunId::new(0));
        let mut opportunities = Self {
            content,
            analysis,
            justify,
            start,
            end,
            ascii,
            spaces,
            simple: false,
            last: end,
            scope: scope.as_ref().and_then(|scope| scope.base),
            per_cluster,
            characters,
            scripts,
            combined,
            near: Cell::new(near),
            columns: scope.map(|scope| RefCell::new(scope.columns)),
        };
        // A class decides only where no ruby column or combined unit says
        // otherwise.
        opportunities.simple = opportunities.ascii
            && opportunities.columns.is_none()
            && !combined
            && opportunities.per_cluster.is_none()
            && matches!(justify, TextJustify::Auto | TextJustify::InterWord);
        if let Some(summary) = summary {
            opportunities.last = summary.last;
            return Some(opportunities);
        }
        let mut at = end;
        while at > start {
            at = ClusterId::new(at.get() - 1);
            if opportunities.kind(at) != Kind::Invisible {
                opportunities.last = at;
                break;
            }
        }
        Some(opportunities)
    }

    /// Returns the line-wide facts a placed line keeps for its readers.
    pub(crate) fn summary(&self) -> JustifySummary {
        JustifySummary {
            ascii: self.ascii,
            spaces: self.spaces,
            last: self.last,
        }
    }

    /// Returns how many opportunities `cluster` has, before and after it:
    /// zero, one or two.
    pub(crate) fn count(&self, cluster: ClusterId) -> u32 {
        let (before, after) = self.at(cluster);
        u32::from(before) + u32::from(after)
    }

    /// Returns whether the line is ASCII and justified at its spaces, so
    /// [`simple_after`](Self::simple_after) answers for each cluster.
    pub(crate) fn is_simple(&self) -> bool {
        self.simple
    }

    /// Returns whether `cluster` has an opportunity after it, on a line that
    /// [`is_simple`](Self::is_simple). It has none before.
    #[inline]
    pub(crate) fn simple_after(&self, cluster: ClusterId) -> bool {
        cluster >= self.start
            && cluster < self.end
            && self.last != cluster
            && self
                .analysis
                .clusters
                .class(cluster)
                .is_some_and(ClusterClass::is_space_or_tab)
    }

    /// Returns how many opportunities the clusters of `range` have.
    pub(crate) fn count_range(&self, range: Range<ClusterId>) -> u32 {
        let from = range.start.max(self.start);
        let to = range.end.min(self.end);
        if from >= to {
            return 0;
        }
        if self.simple {
            // The spaces and tabs, less the line's last where it is one.
            let spaces = if from == self.start && to == self.end {
                self.spaces
            } else {
                let (_, spaces) = count_ascii_spaces(self.bytes(from..to));
                spaces
            };
            debug_assert_eq!(
                spaces,
                self.analysis.clusters.spaces_or_tabs(from..to),
                "an ASCII line's spaces and tabs are its bytes"
            );
            let last = self.last;
            let last = from <= last && last < to && matches!(self.kind(last), Kind::Space);
            let count = u32::try_from(spaces).unwrap_or(u32::MAX);
            return count.saturating_sub(u32::from(last));
        }
        // What precedes the first, then each in turn.
        let mut previous = self.before(from).map(|at| self.kind(at));
        let mut count = 0u32;
        for cluster in (from..to).ids() {
            let kind = self.kind(cluster);
            let (before, after) = self.rule(cluster, kind, previous);
            count = count.saturating_add(u32::from(before) + u32::from(after));
            if kind != Kind::Invisible {
                previous = Some(kind);
            }
        }
        count
    }

    /// Returns the last cluster of `range` with an opportunity, before or
    /// after it.
    pub(crate) fn last(&self, range: Range<ClusterId>) -> Option<ClusterId> {
        if self.simple {
            range
                .ids()
                .rev()
                .find(|&cluster| self.simple_after(cluster))
        } else {
            range.ids().rev().find(|&cluster| self.count(cluster) > 0)
        }
    }

    /// Returns the first cluster of `range` with an opportunity, as
    /// [`last`](Self::last) finds the last.
    pub(crate) fn first(&self, range: Range<ClusterId>) -> Option<ClusterId> {
        if self.simple {
            range.ids().find(|&cluster| self.simple_after(cluster))
        } else {
            range.ids().find(|&cluster| self.count(cluster) > 0)
        }
    }

    /// Returns the text of `clusters` as bytes, empty where they are not
    /// the text's.
    fn bytes(&self, clusters: Range<ClusterId>) -> &'a [u8] {
        let (from, to) = (
            self.analysis.clusters.start(clusters.start),
            self.analysis.clusters.start(clusters.end),
        );
        self.content
            .text
            .as_bytes()
            .get(from.get()..to.get())
            .unwrap_or_default()
    }

    /// Returns whether `cluster` has an opportunity before it and after it.
    ///
    /// Out of line: inlined, it slows a justified ASCII line's glyph walk,
    /// which never calls it, by 9%.
    #[inline(never)]
    pub(crate) fn at(&self, cluster: ClusterId) -> (bool, bool) {
        if cluster < self.start || cluster >= self.end {
            return (false, false);
        }
        let kind = self.kind(cluster);
        // Only an ideograph looks back for the room before it, and under
        // `inter-character` any character, for an atomic inline.
        let previous = if matches!(kind, Kind::Cjk | Kind::UnitStart) || self.characters {
            self.before(cluster).map(|at| self.kind(at))
        } else {
            None
        };
        self.rule(cluster, kind, previous)
    }

    /// Applies the rule to `cluster` of `kind`, after `previous`.
    ///
    /// Under `inter-character`, a run of atomic inlines or cursive letters
    /// takes no room; the character after it takes the room before it.
    fn rule(&self, cluster: ClusterId, kind: Kind, previous: Option<Kind>) -> (bool, bool) {
        let last = self.last == cluster;
        let justify = match &self.per_cluster {
            Some(per_cluster) => per_cluster.justify(self.content, self.analysis, cluster),
            None => self.justify,
        };
        match justify {
            TextJustify::None => (false, false),
            TextJustify::InterWord => (false, kind == Kind::Space && !last),
            TextJustify::InterCharacter => {
                let takes = !matches!(
                    kind,
                    Kind::Invisible | Kind::Object | Kind::Cursive | Kind::UnitStart
                );
                let unit = matches!(previous, Some(Kind::Object | Kind::Cursive));
                (takes && unit, takes && !last)
            }
            TextJustify::Auto => {
                let after = matches!(kind, Kind::Space | Kind::Cjk | Kind::UnitEnd) && !last;
                let before = matches!(kind, Kind::Cjk | Kind::UnitStart)
                    && matches!(previous, Some(Kind::Other | Kind::Object | Kind::Cursive));
                (before, after)
            }
        }
    }

    /// Returns the cluster before `cluster` on the line that its neighbours
    /// see, past the zero-width spaces.
    fn before(&self, cluster: ClusterId) -> Option<ClusterId> {
        let mut at = cluster;
        while at > self.start {
            at = ClusterId::new(at.get() - 1);
            if self.kind(at) != Kind::Invisible {
                return Some(at);
            }
        }
        None
    }

    /// Returns how `cluster` takes part where the ruby column in scope
    /// decides, or `None`.
    fn column_kind(&self, cluster: ClusterId) -> Option<Kind> {
        let mut scope = self.columns.as_ref()?.borrow_mut();
        let column = scope.scope_column(cluster, self.scope)?;
        ruby_kind(scope.columns, column, cluster, self.scope.is_some())
    }

    /// Returns the script run of `runs` holding `cluster`, and moves the
    /// script-run cursor to it.
    fn script_run(&self, runs: &ScriptRuns, cluster: ClusterId) -> Option<ScriptRunId> {
        let end = self.analysis.clusters.end_id();
        let id = runs.cursor_from(self.near.get(), cluster, end)?.id();
        self.near.set(id);
        Some(id)
    }

    /// Returns how `cluster` takes part as part of a combined unit, one
    /// ideograph, or `None` outside one. Moves the script-run cursor.
    fn unit_kind(&self, runs: &ScriptRuns, cluster: ClusterId) -> Option<Kind> {
        let id = self.script_run(runs, cluster)?;
        let run = runs.get(id)?;
        if run.orientation != RunOrientation::Combined {
            return None;
        }
        let first = run.start == cluster;
        let last = runs
            .next_start(id)
            .is_none_or(|next| next == ClusterId::new(cluster.get() + 1));
        Some(match (first, last) {
            (true, true) => Kind::Cjk,
            (true, false) => Kind::UnitStart,
            (false, true) => Kind::UnitEnd,
            (false, false) => Kind::Invisible,
        })
    }

    /// Returns how `cluster` takes part.
    #[inline]
    fn kind(&self, cluster: ClusterId) -> Kind {
        if let Some(kind) = self.column_kind(cluster) {
            return kind;
        }
        if self.combined
            && let Some(kind) = self.unit_kind(&self.analysis.runs, cluster)
        {
            return kind;
        }
        let Some(attrs) = self.analysis.clusters.attrs(cluster) else {
            return Kind::Invisible;
        };
        if let Some(kind) = Kind::from_class(attrs.class()) {
            return kind;
        }
        if self.ascii {
            return Kind::Other;
        }
        let start = self.analysis.clusters.start(cluster).get();
        let text = &self.content.text;
        // Every character that is one of these is past ASCII.
        if text.as_bytes().get(start).is_none_or(u8::is_ascii) {
            return Kind::Other;
        }
        let ch = text
            .get(start..)
            .and_then(|text| text.chars().next())
            .unwrap_or(' ');
        if ch == '\u{A0}' {
            Kind::Space
        } else if is_cjk_ideograph_or_symbol(ch) {
            Kind::Cjk
        } else if self.characters && self.is_cursive(cluster) {
            Kind::Cursive
        } else {
            Kind::Other
        }
    }

    /// Returns whether `cluster` is in a run of a script that
    /// `inter-character` leaves whole: Arabic, Syriac, Mongolian, N'Ko,
    /// Mandaic, Hanifi Rohingya and Phags-pa, as in Chrome 155. Chrome
    /// spreads Adlam, Sogdian, Manichaean and the other joining scripts.
    fn is_cursive(&self, cluster: ClusterId) -> bool {
        if !self.scripts {
            return false;
        }
        let runs = &self.analysis.runs;
        let script = self
            .script_run(runs, cluster)
            .and_then(|id| runs.get(id))
            .map(|run| run.script);
        script.is_some_and(|script| CURSIVE.contains(&script))
    }
}

/// The scripts whose letters `inter-character` leaves whole.
const CURSIVE: [Script; 7] = [
    Script::from_bytes(*b"Arab"),
    Script::from_bytes(*b"Syrc"),
    Script::from_bytes(*b"Mong"),
    Script::from_bytes(*b"Nkoo"),
    Script::from_bytes(*b"Mand"),
    Script::from_bytes(*b"Rohg"),
    Script::from_bytes(*b"Phag"),
];

/// Returns whether `bytes` are ASCII, and how many of them are spaces or
/// tabs.
///
/// It reads eight bytes a step. A space or tab byte becomes zero when xored
/// with it, and each zero byte's high bit is set and counted.
fn count_ascii_spaces(bytes: &[u8]) -> (bool, usize) {
    const LOW: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const SPACES: u64 = 0x2020_2020_2020_2020;
    const TABS: u64 = 0x0909_0909_0909_0909;
    // Set the high bit of each zero byte of `word`, and of no other. Adding
    // 0x7F to a byte's low seven bits carries into its high bit unless they
    // are all clear, and never into the next byte.
    let zeros = |word: u64| !(((word & LOW) + LOW) | word) & HIGH;
    let (words, rest) = bytes.as_chunks::<8>();
    let mut high = 0u64;
    let mut count = 0usize;
    for &word in words {
        work::step();
        let word = u64::from_le_bytes(word);
        high |= word;
        let found = zeros(word ^ SPACES) | zeros(word ^ TABS);
        count += usize::try_from(found.count_ones()).unwrap_or(0);
    }
    let ascii = high & HIGH == 0 && rest.is_ascii();
    if !rest.is_empty() {
        work::step();
    }
    let rest = rest.iter().filter(|&&byte| matches!(byte, b' ' | b'\t'));
    (ascii, count + rest.count())
}

/// Returns how `cluster` takes part where the ruby column `column` decides.
///
/// `column` is what [`ColumnScope::scope_column`] finds. Where `unit`, the
/// walk is in its parent's base, and its base is one ideograph. A
/// base-shorter column is one object. Annotation text is unseen.
fn ruby_kind(
    rubies: &RubyColumns,
    column: &RubyColumn,
    cluster: ClusterId,
    unit: bool,
) -> Option<Kind> {
    let base = column.base.clone();
    if base.contains(&cluster) && unit {
        return Some(
            match (cluster == base.start, cluster.get() + 1 == base.end.get()) {
                (true, true) => Kind::Cjk,
                (true, false) => Kind::UnitStart,
                (false, true) => Kind::UnitEnd,
                (false, false) => Kind::Invisible,
            },
        );
    }
    if base.contains(&cluster) {
        return column
            .is_base_shorter()
            .then_some(if cluster == base.start {
                Kind::Object
            } else {
                Kind::Invisible
            });
    }
    rubies
        .levels(column)
        .iter()
        .any(|level| level.clusters.contains(&cluster))
        .then_some(Kind::Invisible)
}

/// Returns whether `ch` is set on the CJK grid, as Chrome's
/// `Character::IsCJKIdeographOrSymbol` says.
fn is_cjk_ideograph_or_symbol(ch: char) -> bool {
    let c = u32::from(ch);
    if c < 0x2C7 {
        return false;
    }
    // Isolated symbols, as Chrome lists them.
    if matches!(
        c,
        0x2C7
            | 0x2CA
            | 0x2CB
            | 0x2D9
            | 0x2EA
            | 0x2EB
            | 0x2020
            | 0x2021
            | 0x2030
            | 0x203B
            | 0x203C
            | 0x2042
            | 0x2047
            | 0x2048
            | 0x2049
            | 0x2051
            | 0x20DD
            | 0x20DE
            | 0x2100
            | 0x2103
            | 0x2105
            | 0x2109
            | 0x210A
            | 0x2113
            | 0x2116
            | 0x2121
            | 0x212B
            | 0x213B
            | 0x2150
            | 0x2151
            | 0x2152
            | 0x217F
            | 0x2189
            | 0x2307
            | 0x2312
            | 0x23CE
            | 0x2423
            | 0x25A0
            | 0x25A1
            | 0x25A2
            | 0x25AA
            | 0x25AB
            | 0x25B1
            | 0x25B2
            | 0x25B3
            | 0x25B6
            | 0x25B7
            | 0x25BC
            | 0x25BD
            | 0x25C0
            | 0x25C1
            | 0x25C6
            | 0x25C7
            | 0x25C9
            | 0x25CB
            | 0x25CC
            | 0x25EF
            | 0x2605
            | 0x2606
            | 0x260E
            | 0x2616
            | 0x2617
            | 0x2640
            | 0x2642
            | 0x26A0
            | 0x26BD
            | 0x26BE
            | 0x2713
            | 0x271A
            | 0x273F
            | 0x2740
            | 0x2756
            | 0x2B1A
            | 0xFE10
            | 0xFE11
            | 0xFE12
            | 0xFE19
            | 0xFF1D
            | 0x1F100
    ) {
        return true;
    }
    matches!(
        c,
        // Symbols in ranges.
        0x2156..=0x215A
            | 0x2160..=0x216B
            | 0x2170..=0x217B
            | 0x23BE..=0x23CC
            | 0x2460..=0x2492
            | 0x249C..=0x24FF
            | 0x25CE..=0x25D3
            | 0x25E2..=0x25E6
            | 0x2600..=0x2603
            | 0x2660..=0x266F
            | 0x2672..=0x267D
            | 0x2776..=0x277F
            // Ideographic description, CJK symbols and punctuation but the
            // wavy dash, hiragana, katakana, bopomofo and its extension,
            // enclosed CJK letters and the CJK compatibility block.
            | 0x2FF0..=0x302F
            | 0x3031..=0x312F
            | 0x3190..=0x31BF
            | 0x3200..=0x33FF
            | 0xF860..=0xF862
            // CJK compatibility forms, and the half- and full-width forms
            // but the hyphen-minus, the semicolon and the angle brackets.
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF0C
            | 0xFF0E..=0xFF1A
            | 0xFF1F..=0xFFEF
            // Enclosed and squared letters, and the emoji Chrome counts.
            | 0x1F110..=0x1F129
            | 0x1F130..=0x1F149
            | 0x1F150..=0x1F169
            | 0x1F170..=0x1F189
            | 0x1F200..=0x1F6FF
            // The ideographs: radicals, strokes, the unified ideographs and
            // their extensions, and the compatibility ideographs.
            | 0x2E80..=0x2FDF
            | 0x31C0..=0x31EF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
            | 0x30000..=0x323AF
    )
}

#[cfg(test)]
mod tests {
    use super::count_ascii_spaces;

    /// The word-at-a-time count matches a byte-at-a-time one for every byte
    /// in every place.
    #[test]
    fn ascii_spaces_and_tabs_are_found_a_word_at_a_time() {
        let mut bytes = [b'a'; 19];
        for byte in 0..=u8::MAX {
            for at in 0..bytes.len() {
                bytes.fill(b'a');
                bytes[(at + 7) % 19] = b' ';
                bytes[(at + 9) % 19] = b'\t';
                bytes[at] = byte;
                for len in [0, 7, 8, 9, 16, 19] {
                    let part = &bytes[..len];
                    let each = part.iter().filter(|&&b| b == b' ' || b == b'\t').count();
                    assert_eq!(
                        count_ascii_spaces(part),
                        (part.is_ascii(), each),
                        "{part:?}"
                    );
                }
            }
        }
    }
}
