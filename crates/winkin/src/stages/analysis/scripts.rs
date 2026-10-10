//! Script resolution and the runs.
//!
//! A cluster's script is its first character's, resolved as UAX #24 and
//! Blink's `ScriptRunIterator` resolve it:
//!
//! - **Common, Inherited and Unknown** take the preceding real script, or at
//!   a run's start the following one. The runs are held in scratch until
//!   their script is known, so this is one look-ahead, not a second pass.
//! - **Script_Extensions** narrow a shared character toward its neighbour: a
//!   character that may be Hiragana, Katakana or Han stays in a Han run, and
//!   characters that may each be several scripts keep the scripts they all
//!   share. One that fits neither side starts a run of its own.
//! - **Paired brackets** take the script of their opener, so the closing
//!   parenthesis after Japanese quoted in English is English.
//!
//! As in Blink, the resolution runs over the whole text, across paragraphs:
//! a paragraph of digits after a paragraph of Han is Han, which is the script
//! font fallback then asks for. The runs themselves never cross a paragraph.

use alloc::vec::Vec;

use parlance::Script;

use super::{
    BidiLevel, ClusterId, ParagraphFlags, Paragraphs, RunOrientation, ScriptRun, ScriptRuns,
};
use crate::stages::content::LanguageId;
use crate::unicode::{self, BracketPairId, CoreProps, ScriptId};

/// Brackets remembered at once. Blink keeps 32; deeper nesting forgets the
/// outermost, which then closes as Common.
const MAX_BRACKETS: usize = 32;

/// A run found by the writer, before its paragraph's level is known and
/// perhaps before its script is.
#[derive(Copy, Clone, Debug)]
pub(super) struct PendingRun {
    start: ClusterId,
    language: LanguageId,
    /// Its script, or Common until the script run it is in resolves.
    script: ScriptId,
    orientation: RunOrientation,
    /// Its clusters stand as its script run does, which is still undecided:
    /// `orientation` is upright until it resolves.
    follows: bool,
}

/// What a cluster does to the script run it meets.
enum ScriptStep {
    /// It joins the run, which keeps its script or lack of one.
    Joins,
    /// It joins the run and decides its script, which the run's clusters
    /// before it take too.
    Resolves(ScriptId),
    /// It cannot join: the run ends, with this script, and a run starts at
    /// the cluster.
    Breaks(ScriptId),
}

/// The script run the clusters are in, and the brackets open.
struct ScriptState {
    /// The run's script, once a character has decided it.
    script: Option<ScriptId>,
    /// While undecided: the Script_Extensions the run's shared characters
    /// allow, the first such character's, with a bit per script still
    /// allowed by every one since. Empty when none has come.
    candidates: &'static [ScriptId],
    allowed: u64,
    /// Open brackets, innermost last: each's pair, and the script of the run
    /// it opened in, Common while that was undecided.
    brackets: [(BracketPairId, ScriptId); MAX_BRACKETS],
    depth: usize,
    /// The first bracket opened while the run is undecided, whose script the
    /// run's resolution fills in.
    undecided_from: usize,
}

impl ScriptState {
    fn new() -> Self {
        Self {
            script: None,
            candidates: &[],
            allowed: 0,
            brackets: [(BracketPairId::default(), ScriptId::COMMON); MAX_BRACKETS],
            depth: 0,
            undecided_from: 0,
        }
    }

    /// What the cluster whose first character is `props`'s does.
    ///
    /// Every paired bracket is paired, whatever its own script, as Blink
    /// pairs them: a closing bracket that closes an opener is the opener's
    /// script, and an opener takes the script of the run it opens in, which
    /// its own script decides first.
    fn step(&mut self, props: CoreProps) -> ScriptStep {
        let bracket = props.bracket_pair();
        let closes = match bracket {
            Some(pair) if !props.opens_bracket() => self.close(pair),
            _ => None,
        };
        let step = match (closes, props.scripts().single()) {
            (Some(script), _) => self.real(script),
            (None, Some(script)) if is_shared(script) => ScriptStep::Joins,
            (None, Some(script)) => self.real(script),
            (None, None) => self.shared(unicode::script_extensions(props), unicode::script(props)),
        };
        if let Some(pair) = bracket
            && props.opens_bracket()
        {
            self.open(pair);
        }
        step
    }

    /// A character of one real script.
    fn real(&mut self, script: ScriptId) -> ScriptStep {
        match self.script {
            Some(current) if current == script => ScriptStep::Joins,
            Some(_) => self.restart(Some(script), &[]),
            None if !self.candidates.is_empty() && !self.allows(script) => {
                self.restart(Some(script), &[])
            }
            None => {
                self.resolve(script);
                ScriptStep::Resolves(script)
            }
        }
    }

    /// A character that may be any of `scripts`, whose own Script is `own`.
    fn shared(&mut self, scripts: &'static [ScriptId], own: ScriptId) -> ScriptStep {
        if scripts.is_empty() {
            // A set of nothing is no constraint; the table has none, but it
            // costs nothing to be sure.
            return if is_shared(own) {
                ScriptStep::Joins
            } else {
                self.real(own)
            };
        }
        match self.script {
            Some(current) if scripts.contains(&current) => ScriptStep::Joins,
            Some(_) => self.restart(None, scripts),
            None if self.candidates.is_empty() => {
                self.candidates = scripts;
                self.allowed = all(scripts);
                ScriptStep::Joins
            }
            None => {
                let allowed = self.allowed & within(self.candidates, scripts);
                if allowed == 0 {
                    self.restart(None, scripts)
                } else {
                    self.allowed = allowed;
                    ScriptStep::Joins
                }
            }
        }
    }

    /// Whether the cluster whose first character is `props`'s joins the run
    /// and changes nothing, as [`step`](Self::step) would find.
    ///
    /// That holds for a cluster with no paired bracket and one script,
    /// shared or the run's own. The Latin-1 fast path passes most clusters
    /// through the runs by this test.
    #[inline]
    fn joins_quietly(&self, props: CoreProps) -> bool {
        props.bracket_pair().is_none()
            && props
                .scripts()
                .single()
                .is_some_and(|script| is_shared(script) || self.script == Some(script))
    }

    /// Whether the candidates still allow `script`.
    fn allows(&self, script: ScriptId) -> bool {
        self.candidates
            .iter()
            .take(64)
            .enumerate()
            .any(|(bit, &candidate)| candidate == script && self.allowed & (1 << bit) != 0)
    }

    /// The run's script as it stands: decided, or its first allowed
    /// candidate, or Common.
    fn settled(&self) -> ScriptId {
        match self.script {
            Some(script) => script,
            None => self
                .candidates
                .iter()
                .take(64)
                .enumerate()
                .find(|&(bit, _)| self.allowed & (1 << bit) != 0)
                .map_or(ScriptId::COMMON, |(_, &script)| script),
        }
    }

    /// Ends the run and starts another, decided as `script` or undecided
    /// among `candidates`, and says what the ended run was.
    fn restart(&mut self, script: Option<ScriptId>, candidates: &'static [ScriptId]) -> ScriptStep {
        let ended = self.settled();
        self.script = script;
        self.candidates = candidates;
        self.allowed = all(candidates);
        self.undecided_from = self.depth;
        ScriptStep::Breaks(ended)
    }

    /// Decides the run as `script`, and the brackets it opened undecided.
    fn resolve(&mut self, script: ScriptId) {
        self.script = Some(script);
        let depth = self.depth;
        if let Some(open) = self.brackets.get_mut(self.undecided_from..depth) {
            for (_, opened) in open {
                *opened = script;
            }
        }
        self.undecided_from = depth;
    }

    /// An opening bracket, remembered with the run's script.
    fn open(&mut self, pair: BracketPairId) {
        if self.depth == MAX_BRACKETS {
            // Forget the outermost.
            self.brackets.copy_within(1.., 0);
            self.depth -= 1;
            self.undecided_from = self.undecided_from.saturating_sub(1);
        }
        let script = self.script.unwrap_or(ScriptId::COMMON);
        if let Some(slot) = self.brackets.get_mut(self.depth) {
            *slot = (pair, script);
            self.depth += 1;
        }
    }

    /// A closing bracket: the script its opener took, if it has an opener and
    /// that was decided. The opener and everything opened since are closed.
    fn close(&mut self, pair: BracketPairId) -> Option<ScriptId> {
        let open = self.brackets.get(..self.depth)?;
        let at = open.iter().rposition(|&(open, _)| open == pair)?;
        let (_, script) = *open.get(at)?;
        self.depth = at;
        self.undecided_from = self.undecided_from.min(at);
        (!is_shared(script)).then_some(script)
    }
}

/// Whether `script` is one every script shares: Common, Inherited, or
/// Unknown for what has none.
pub(super) fn is_shared(script: ScriptId) -> bool {
    script == ScriptId::COMMON || script == ScriptId::INHERITED || script == ScriptId::UNKNOWN
}

/// Whether `script` is vertical-only, written only down the line, which CSS
/// Writing Modes 4 names: Mongolian and Phags-pa. Its characters lie on their
/// side in a vertical line, even under `text-orientation: upright`.
pub(super) fn is_vertical_only(script: ScriptId) -> bool {
    matches!(&script.tag(), b"Mong" | b"Phag")
}

/// How a cluster that stands as its script run does stands in a run of
/// `script`: on its side in a vertical-only script, and upright otherwise.
fn following(script: ScriptId) -> RunOrientation {
    if is_vertical_only(script) {
        RunOrientation::Sideways
    } else {
        RunOrientation::Upright
    }
}

/// A bit for each of `scripts`, up to 64.
fn all(scripts: &[ScriptId]) -> u64 {
    match scripts.len() {
        0 => 0,
        n if n >= 64 => u64::MAX,
        n => (1 << n) - 1,
    }
}

/// A bit for each of `candidates` that is also in `scripts`.
fn within(candidates: &[ScriptId], scripts: &[ScriptId]) -> u64 {
    candidates
        .iter()
        .take(64)
        .enumerate()
        .filter(|(_, candidate)| scripts.contains(candidate))
        .fold(0, |bits, (bit, _)| bits | (1 << bit))
}

/// How a cluster is set, as the runs read it: its orientation, and the
/// combined unit it is in.
///
/// It takes four bytes, which fit in a pending cluster's padding. The
/// cluster loop of every layout copies one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Setting {
    pub(super) orientation: RunOrientation,
    /// The combined unit it is in, a number the writer gives each unit in
    /// turn, or 0 in none. Only a cluster's neighbours are asked whether
    /// they share its unit, so the numbers go round, and two units side by
    /// side never share one.
    pub(super) unit: u16,
    /// It stands as the script run it is in does: on its side in a
    /// vertical-only script, and upright otherwise. `orientation` is upright
    /// until the runs know that script.
    pub(super) follows: bool,
}

impl Setting {
    /// Set across a horizontal line, in no unit.
    pub(super) const HORIZONTAL: Self = Self {
        orientation: RunOrientation::Horizontal,
        unit: 0,
        follows: false,
    };

    /// Resolves the setting in a script run of `script`: a cluster that
    /// follows its run stands as the run does.
    fn resolve(self, script: ScriptId) -> Self {
        if self.follows {
            Self {
                orientation: following(script),
                follows: false,
                ..self
            }
        } else {
            self
        }
    }
}

/// Builds the runs, a cluster at a time, into the scratch list.
pub(super) struct RunBuilder {
    state: ScriptState,
    /// The language of the run open, or `None` before the first.
    language: Option<LanguageId>,
    /// How the last cluster is set: horizontally before the first, which
    /// opens its paragraph and so a run whatever it is.
    setting: Setting,
    /// The first run in the list of the script run the clusters are in.
    from: usize,
}

impl RunBuilder {
    pub(super) fn new() -> Self {
        Self {
            state: ScriptState::new(),
            language: None,
            setting: Setting::HORIZONTAL,
            from: 0,
        }
    }

    /// Adds cluster `id` to the runs and returns the orientation of the run
    /// it starts, if it starts one.
    ///
    /// `props` is its first character's data, `language` its language and
    /// `setting` how it is set. `opens_paragraph` says it is its
    /// paragraph's first cluster.
    ///
    /// A run ends where the orientation changes, and around each combined
    /// unit. A unit is one run whatever scripts it holds, since it is set as
    /// one character. Its script is the one the script run it starts in
    /// resolves to. Scripts still resolve across it as everywhere else.
    ///
    /// A cluster that follows its script run stands as that run does. While
    /// the run is undecided it is held upright, and the runs it is in turn
    /// when the run resolves. The orientation returned is then upright.
    pub(super) fn cluster(
        &mut self,
        runs: &mut Vec<PendingRun>,
        id: ClusterId,
        props: CoreProps,
        language: LanguageId,
        setting: Setting,
        opens_paragraph: bool,
    ) -> Option<RunOrientation> {
        let step = self.state.step(props);
        let breaks = match step {
            ScriptStep::Breaks(ended) => {
                settle(runs, self.from, ended);
                self.from = runs.len();
                true
            }
            ScriptStep::Resolves(script) => {
                settle(runs, self.from, script);
                self.setting = self.setting.resolve(script);
                false
            }
            ScriptStep::Joins => false,
        };
        let setting = match self.state.script {
            Some(script) => setting.resolve(script),
            None => setting,
        };
        // Inside a combined unit, only the unit's own start starts a run.
        let within = setting.unit != 0 && self.setting.unit == setting.unit;
        let restarts = if within {
            opens_paragraph
        } else {
            breaks || opens_paragraph || self.language != Some(language) || self.setting != setting
        };
        self.setting = setting;
        restarts.then(|| {
            runs.push(PendingRun {
                start: id,
                language,
                script: self.state.script.unwrap_or(ScriptId::COMMON),
                orientation: setting.orientation,
                follows: setting.follows,
            });
            self.language = Some(language);
            setting.orientation
        })
    }

    /// Whether [`cluster`](Self::cluster) would change nothing for a
    /// horizontal cluster in `language` that does not open its paragraph.
    ///
    /// That holds if it joins the script run quietly and the open run has
    /// its language and setting. The Latin-1 fast path asks this before
    /// `cluster`.
    #[inline]
    pub(super) fn joins(&self, props: CoreProps, language: LanguageId) -> bool {
        self.language == Some(language)
            && self.setting == Setting::HORIZONTAL
            && self.state.joins_quietly(props)
    }

    /// The text ended: the last script run settles.
    pub(super) fn finish(&mut self, runs: &mut [PendingRun]) {
        settle(runs, self.from, self.state.settled());
    }
}

/// Gives the runs from `from` on the script `script`, and stands those that
/// follow it as it does.
fn settle(runs: &mut [PendingRun], from: usize, script: ScriptId) {
    if let Some(runs) = runs.get_mut(from..) {
        for run in runs {
            run.script = script;
            if run.follows {
                run.orientation = following(script);
                run.follows = false;
            }
        }
    }
}

/// Writes the runs out, each at its clusters' level.
///
/// A run takes its paragraph's level. In a paragraph with clusters at other
/// levels, runs are split wherever `changes` says the level changes. So a
/// run has one level, and shaping sets it in one direction. `end` is the
/// text's end, where the last run ends.
pub(super) fn emit(
    runs: &[PendingRun],
    paragraphs: &Paragraphs,
    changes: &[(ClusterId, BidiLevel)],
    end: ClusterId,
    out: &mut ScriptRuns,
) {
    // A run a pending one, and one more a change inside one.
    out.reserve(runs.len() + changes.len());
    let mut open = paragraphs.iter().peekable();
    let mut changes = changes.iter().copied().peekable();
    // The level the last change read set, which goes on into the runs
    // after it: a mixed paragraph's first cluster has a change.
    let mut current = BidiLevel::LTR;
    let mut runs = runs.iter().peekable();
    while let Some(run) = runs.next() {
        while open
            .peek()
            .is_some_and(|&(id, _)| paragraphs.clusters(id).end <= run.start)
        {
            open.next();
        }
        let (level, mixed) = open
            .peek()
            .map_or((BidiLevel::LTR, false), |(_, paragraph)| {
                (
                    paragraph.level,
                    paragraph.flags.contains(ParagraphFlags::MIXED_LEVELS),
                )
            });
        let mut push = |start: ClusterId, level: BidiLevel| {
            let pushed = out.push(ScriptRun {
                start,
                script: Script::from_bytes(run.script.tag()),
                language: run.language,
                level,
                orientation: run.orientation,
            });
            debug_assert!(pushed.is_some(), "no more runs than clusters");
        };
        if !mixed {
            push(run.start, level);
            continue;
        }
        // The level at its start: the last change at or before it, which is
        // in its paragraph, whose first cluster has one.
        while let Some((_, changed)) = changes.next_if(|&(at, _)| at <= run.start) {
            current = changed;
        }
        push(run.start, current);
        // Where the run ends: the next one's start, or the text's end.
        let end = runs.peek().map_or(end, |next| next.start);
        while let Some((at, changed)) = changes.next_if(|&(at, _)| at < end) {
            push(at, changed);
            current = changed;
        }
    }
}
