//! Justification opportunities: where a justified line puts its extra room,
//! cluster by cluster.
//!
//! Three readers ask the one rule:
//! - this stage, when it sizes a ruby column's base, whose room
//!   `ruby-align` spreads over the base's opportunities;
//! - line layout, when it counts a line's opportunities and gives each
//!   piece its share;
//! - the readers, when they place a justified line's glyphs and clusters.
//!
//! So what a reader walks adds up to what line layout placed. The rule
//! lives in this stage, the first to ask it, so no stage names a later
//! one's rule. Nothing is stored per cluster. Line layout records a line's
//! amount per opportunity and which opportunity takes the remainder; which
//! clusters take room is this rule, asked again.
//!
//! **Chrome's rule** (`Character::ExpansionOpportunityCount` and
//! `ShapeResultSpacing::ComputeSpacing`, over the line's text less what
//! hangs at its end), under `text-justify: auto`:
//! - a space, a tab or a no-break space (Chrome's `TreatAsSpace`, U+00A0
//!   and not U+2007 or U+202F) has an opportunity after it;
//! - an ideograph or a CJK symbol has one after it, and one before it where
//!   what precedes it on the line is neither. Chrome's
//!   `IsCJKIdeographOrSymbol` counts the ideographs, kana, bopomofo, the CJK
//!   symbols and punctuation, the full-width forms, and a few symbols set on
//!   the same grid;
//! - nothing before the line's first character: the line starts as if after
//!   an opportunity;
//! - nothing after its last, where that would be one: the last opportunity
//!   is dropped;
//! - what Chrome treats as a zero-width space (a control, U+200B, a soft
//!   hyphen, a lone joiner) takes none, and its neighbours don't see it.
//!
//! `inter-word` keeps the spaces alone; `inter-character` puts an
//! opportunity after every cluster but the line's last, as Chrome's
//! `distribute` does, and one before the first character after an atomic
//! inline or a run of them, which is one unit; `none` justifies nothing.
//! Each cluster follows its own text's `text-justify`, as Chrome reads it
//! item by item: a span under `none` takes no room on a justified line. A
//! line with no opportunity is set at its start.
//!
//! Hangul is not among the ideographs, as in Chrome, so Korean justifies at
//! its spaces.
//!
//! **Combined text** (`text-combine-upright`) is one character. Blink
//! justifies a `LayoutTextCombine` as U+3042, a hiragana
//! (`kTextCombineItemMarker`, `JustifyResults`). It has an ideograph's
//! opportunity before it, on its first cluster, which moves its whole em
//! along, and one after it, on its last. The clusters between are unseen.
//!
//! **Ruby on a justified line** follows Chrome too (`JustifyResults`):
//! - a column's base is justified as the line's own text where it is at
//!   least as wide as its annotations;
//! - where an annotation is wider, the column is one object, as an atomic
//!   inline's U+FFFC is (`kBaseShorterRubyMarker`), and `ruby-align` alone
//!   spreads its base;
//! - an annotation's text is on a line of its own, and the line doesn't see
//!   it.

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
    /// An ideograph or a CJK symbol: one after it, and one before it where
    /// what precedes it is neither this nor a space.
    Cjk,
    /// What Chrome treats as a zero-width space: nothing, and not seen by
    /// its neighbours.
    Invisible,
    /// An atomic inline's U+FFFC: nothing, even between characters.
    Object,
    /// A letter of a cursive script, whose letters join: under
    /// `inter-character`, an atomic inline's nothing; otherwise anything
    /// else's.
    Cursive,
    /// The first cluster of a combined unit of several: an ideograph's
    /// opportunity before it, none after.
    UnitStart,
    /// The last cluster of a combined unit of several: an ideograph's
    /// opportunity after it, none before, and seen as an ideograph.
    UnitEnd,
    /// Anything else.
    Other,
}

impl Kind {
    /// Returns how a cluster of `class` takes part, where its class decides.
    ///
    /// Returns `None` for a no-break space, text, an emoji, a symbol or
    /// another space separator, where the character decides.
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

/// Line-wide facts computed once during placement and reused by readers.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct JustifySummary {
    ascii: bool,
    spaces: usize,
    last: ClusterId,
}

/// The opportunities of one line: which of its clusters take a share of its
/// extra room, and where.
///
/// They are worked out as they are read, from each cluster's class. Where
/// the line's text is ASCII, as Latin prose nearly always is, the class
/// decides and the text is not read. The line's last cluster that its
/// neighbours see is found once, since the opportunity after it is dropped.
///
/// **Counted from the text's bytes** where the line is ASCII and justified
/// at its spaces ([`is_simple`](Self::is_simple)). There every character is
/// a cluster of its own, whose class is a space or a tab exactly where the
/// character is one. So the line's bytes are its classes, and its
/// opportunities are counted a machine word at a time.
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
    /// Where it is ASCII, how many of its clusters are spaces or tabs. One
    /// walk over its bytes counts them and tests for ASCII.
    spaces: usize,
    /// The line is ASCII and justified at its spaces (`auto` or
    /// `inter-word`), so a cluster's class decides
    /// ([`is_simple`](Self::is_simple)).
    ///
    /// A cluster then has an opportunity after it where its class is a
    /// space or a tab and it is not the last, and none before it.
    simple: bool,
    /// The line's last cluster that its neighbours see, whose opportunity
    /// after is dropped. `end` where there is none.
    last: ClusterId,
    scope: Option<RubyColumnId>,
    /// Each cluster's own `text-justify`, where some style sets one other
    /// than `auto`; `justify` answers for every cluster otherwise.
    per_cluster: Option<ClusterJustify>,
    /// Some cluster may be justified under `inter-character`.
    characters: bool,
    /// The script runs are read: some text is combined or, under
    /// `inter-character`, some letter may be cursive.
    scripts: bool,
    /// Some text is combined. Each combined unit is one character.
    combined: bool,
    /// The script run and the ruby column the last cluster's kind was read
    /// in.
    ///
    /// The next cluster is read near it: a line's clusters are read in
    /// order, and a cluster's neighbours a few back. So each step moves a
    /// run or a column at a time. They are sought once, at the line's
    /// start, where the line has either.
    near: Cell<ScriptRunId>,
    columns: Option<RefCell<ColumnScope<'a>>>,
}

/// The `text-justify` of each of a line's clusters, where they may differ.
///
/// The item cursor moves from the last cluster read, as `near` does, so a
/// line read in order seeks no item.
#[derive(Clone)]
struct ClusterJustify {
    /// The first-line variant the line is set in, whose styles it reads.
    variant: FirstLineVariant,
    item: Cell<ItemId>,
}

impl ClusterJustify {
    /// Returns the `text-justify` of the text holding `cluster`, of
    /// `content` and `analysis`.
    fn justify(&self, content: &Content, analysis: &Analysis, cluster: ClusterId) -> TextJustify {
        let items = &analysis.item_clusters;
        // The first item at the cluster's start may be a box's edge: the
        // cluster's own item is the first after it that holds clusters.
        let mut item = items.walk_to(self.item.get(), cluster);
        while items.range(item).end <= cluster && item.get() + 1 < content.items.len() {
            work::step();
            item = ItemId::new(item.get() + 1);
        }
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
    /// Returns the opportunities of `clusters` as a line of their own under
    /// `justify`.
    ///
    /// The clusters are a ruby base or annotation, which Chrome justifies as
    /// a line of its own (`ApplyJustification` for `kRubyBase` and
    /// `kRubyText`). Returns `None` where `text-justify: none` makes none.
    pub(crate) fn new(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
    ) -> Option<Self> {
        Self::from_clusters(content, analysis, clusters, justify, None, None, None)
    }

    /// Returns the opportunities of a ruby base.
    ///
    /// The base sees its direct child columns as indivisible characters, and
    /// never counts their annotation clusters as opportunities. The columns
    /// are walked to from `near`, any column near `clusters`' start.
    pub(crate) fn from_base(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        rubies: &'a RubyColumns,
        (column, near): (RubyColumnId, RubyColumnId),
    ) -> Option<Self> {
        let scope = ColumnScope::from_column(rubies, near, clusters.start);
        let mut found = Self::from_clusters(
            content,
            analysis,
            clusters.clone(),
            justify,
            Some(scope),
            None,
            None,
        )?;
        found.scope = Some(column);
        found.last = clusters.end;
        for at in clusters.ids().rev() {
            if found.kind(at) != Kind::Invisible {
                found.last = at;
                break;
            }
        }
        Some(found)
    }

    /// Restores a base walk using the summary counted when it opened, its
    /// columns walked to from `near`.
    pub(crate) fn from_base_summary(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        rubies: &'a RubyColumns,
        (column, near): (RubyColumnId, RubyColumnId),
        summary: JustifySummary,
    ) -> Option<Self> {
        let scope = ColumnScope::from_column(rubies, near, clusters.start);
        let mut found = Self::from_clusters(
            content,
            analysis,
            clusters,
            justify,
            Some(scope),
            Some(summary),
            None,
        )?;
        found.scope = Some(column);
        Some(found)
    }

    /// Returns the opportunities of a line set in `stages`' variant, each
    /// cluster under its own text's `text-justify`.
    ///
    /// `clusters` runs from the line's first cluster to its content's end.
    /// Ruby columns come from the text the line is measured in, where the
    /// content has any. Returns `None` where every style is `auto` but the
    /// block's `text-justify: none` makes none. Where the line was placed,
    /// the `summary` made then saves a walk over it.
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
        let scope = ruby.then(|| ColumnScope::new(rubies, clusters.start));
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
        // With styles of their own, the clusters decide, whatever the
        // block's value.
        let justify = match per_cluster {
            Some(_) => TextJustify::Auto,
            None => content.facts.text(block).justify,
        };
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
    /// through `scope`, which starts at their first, where given, and each
    /// cluster's `text-justify` through `per_cluster`, where given.
    fn from_clusters(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        scope: Option<ColumnScope<'a>>,
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
            || text.as_bytes().get(bytes).map_or((false, 0), scan),
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
            scope: None,
            per_cluster,
            characters,
            scripts,
            combined,
            near: Cell::new(near),
            columns: scope.map(RefCell::new),
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

    /// Returns whether the line is ASCII and justified at its spaces.
    ///
    /// [`simple_after`](Self::simple_after) then answers for any of its
    /// clusters.
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

    /// Returns how many opportunities the clusters of `range` have, in one
    /// walk.
    pub(crate) fn count_range(&self, range: Range<ClusterId>) -> u32 {
        let from = range.start.max(self.start);
        let to = range.end.min(self.end);
        if from >= to {
            return 0;
        }
        if self.simple {
            // Count the spaces and tabs, less the line's last where it is
            // one. Each character is a cluster, so count bytes. The whole
            // line's count is ready where the whole line is asked.
            let spaces = if from == self.start && to == self.end {
                self.spaces
            } else {
                let (_, spaces) = scan(self.bytes(from..to));
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
        // Find what precedes the first as its neighbours see it, then walk
        // each in turn.
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
    ///
    /// On a line justified at its spaces, it is the last space or tab other
    /// than the line's last, found from the class alone.
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

    /// Returns the text of `clusters` as bytes, or `None` where they are not
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

    /// Returns whether `cluster` has an opportunity before it, which moves
    /// its glyphs along, and one after it, which moves what follows.
    ///
    /// It stays out of line: inlined into a reader's cluster step, it slows
    /// the glyph walk of a justified ASCII line, which never calls it, by 9%.
    #[inline(never)]
    pub(crate) fn at(&self, cluster: ClusterId) -> (bool, bool) {
        if cluster < self.start || cluster >= self.end {
            return (false, false);
        }
        let kind = self.kind(cluster);
        // Only an ideograph looks back for the room before it, and under
        // `inter-character` any character, for an atomic inline.
        let previous = if matches!(kind, Kind::Cjk | Kind::UnitStart) || self.looks_back() {
            self.before(cluster).map(|at| self.kind(at))
        } else {
            None
        };
        self.rule(cluster, kind, previous)
    }

    /// Applies the rule to `cluster` of `kind`, after `previous`, what its
    /// neighbours see before it.
    ///
    /// Under `inter-character`, a run of atomic inlines or of cursive
    /// letters is one unit with no room of its own: the character after it
    /// takes the room before it, as Chrome's text item after an atomic
    /// inline does.
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

    /// Returns whether a cluster other than an ideograph may take room
    /// before it: where `inter-character` may apply.
    fn looks_back(&self) -> bool {
        self.characters
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

    /// Returns how the ruby column held by the cursor says `cluster` takes
    /// part.
    fn column_kind(&self, cluster: ClusterId) -> Option<Kind> {
        let mut scope = self.columns.as_ref()?.borrow_mut();
        let column = scope.scope_column(cluster, self.scope)?;
        ruby_kind(scope.columns, column, cluster, self.scope.is_some())
    }

    /// Returns the script run of `runs` holding `cluster`, and moves the
    /// script-run cursor to it.
    fn script_run(&self, runs: &ScriptRuns, cluster: ClusterId) -> Option<ScriptRunId> {
        let mut id = self.near.get();
        while id.get() > 0 && runs.get(id).is_some_and(|run| run.start > cluster) {
            work::step();
            id = ScriptRunId::new(id.get() - 1);
        }
        while runs.next_start(id).is_some_and(|next| next <= cluster) {
            work::step();
            id = ScriptRunId::new(id.get() + 1);
        }
        self.near.set(id);
        runs.get(id).filter(|run| run.start <= cluster).map(|_| id)
    }

    /// Returns how `cluster` takes part where it is in a combined unit of
    /// `runs`. Moves the script-run cursor to the run holding it.
    ///
    /// A combined unit is one ideograph. Its first cluster takes the room
    /// before it, its last the room after it, and those between are unseen.
    /// A unit of one cluster is an ideograph. A cluster in no unit gets
    /// `None`.
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

    /// Returns whether `cluster` is in a run of a script whose letters join,
    /// which `inter-character` leaves whole.
    ///
    /// The scripts are those Chrome 155 leaves whole: Arabic, Syriac,
    /// Mongolian, N'Ko, Mandaic, Hanifi Rohingya and Phags-pa. It spreads
    /// Adlam, Sogdian, Manichaean and the other joining scripts.
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
/// On an ASCII line, that count is how many clusters have a space's or a
/// tab's class. It reads eight bytes a step. A word is ASCII where no byte
/// has its high bit set. Its spaces are the bytes that become zero once the
/// word is xored with eight spaces, and the same for tabs. Each zero byte is
/// marked by its high bit, and the marks are counted.
fn scan(bytes: &[u8]) -> (bool, usize) {
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
/// `column` is the last column of `rubies` whose base starts at or before
/// `cluster`.
/// - An annotation's text is on a line of its own, so it is unseen.
/// - Where an annotation is wider than its base, the base's first cluster
///   is one object and the rest are unseen.
/// - Anything else gets `None`.
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
///
/// Such characters take room either side when a line is justified.
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
    use super::scan;

    /// The eight-bytes-a-step test and count match a byte-at-a-time one, for
    /// every byte in every place of a word and past it, beside a space and a
    /// tab.
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
                    assert_eq!(scan(part), (part.is_ascii(), each), "{part:?}");
                }
            }
        }
    }
}
