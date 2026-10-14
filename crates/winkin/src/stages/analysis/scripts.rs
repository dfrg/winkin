//! Script resolution and the runs.
//!
//! A cluster's script is its first character's, resolved as UAX #24 and
//! Blink's `ScriptRunIterator` resolve it:
//!
//! - **Common, Inherited and Unknown** take the preceding real script, or
//!   the following one at a run's start. Runs wait in scratch until their
//!   script is known, so this is one look-ahead, not a second pass.
//! - **Script_Extensions** narrow a shared character toward its neighbour.
//!   A character that may be Hiragana, Katakana or Han stays in a Han run;
//!   characters that may each be several scripts keep the scripts they all
//!   share. One that fits neither side starts a run.
//! - **Paired brackets** take their opener's script, so the closing
//!   parenthesis after Japanese quoted in English is English.
//!
//! As in Blink, resolution runs across paragraphs: a paragraph of digits
//! after one of Han is Han, the script font fallback then asks for. Runs
//! never cross a paragraph.

use alloc::vec::Vec;

use parlance::Script;

use super::{
    BidiLevel, ClusterId, ParagraphFlags, Paragraphs, RunOrientation, ScriptRun, ScriptRuns,
};
use crate::stages::content::LanguageId;
use crate::unicode::{self, BracketPairId, CoreProps, ScriptId};

/// The number of open brackets remembered, as in Blink. Deeper nesting
/// forgets the outermost, which then closes as Common.
const MAX_BRACKETS: usize = 32;

/// A run the writer found, before its level and perhaps its script are
/// known.
#[derive(Copy, Clone, Debug)]
pub(super) struct PendingRun {
    start: ClusterId,
    language: LanguageId,
    /// Its script, or Common until its script run resolves.
    script: ScriptId,
    orientation: RunOrientation,
    /// Whether its clusters stand as its still undecided script run does;
    /// `orientation` is upright until the run resolves.
    follows: bool,
}

/// What a cluster does to the script run it meets.
enum ScriptStep {
    /// It joins the run, which keeps its script or lack of one.
    Joins,
    /// It joins the run and decides its script, for the run's earlier
    /// clusters too.
    Resolves(ScriptId),
    /// It cannot join: the run ends with this script, and a new run starts
    /// at the cluster.
    Breaks(ScriptId),
}

/// The current script run and the open brackets.
struct ScriptState {
    /// The run's script, once a character has decided it.
    script: Option<ScriptId>,
    /// While undecided, the first shared character's Script_Extensions;
    /// empty before one comes.
    candidates: &'static [ScriptId],
    /// A bit per entry of `candidates` that every shared character since
    /// allows.
    allowed: u64,
    /// The open brackets, innermost last: each one's pair and the script of
    /// the run it opened in, Common while that is undecided.
    brackets: [(BracketPairId, ScriptId); MAX_BRACKETS],
    depth: usize,
    /// The first bracket opened while the run is undecided; resolving the
    /// run fills in its script and those after it.
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

    /// Returns what the cluster whose first character has `props` does.
    ///
    /// Every paired bracket pairs, whatever its own script, as in Blink. A
    /// closing bracket that closes an opener takes the opener's script. An
    /// opener's own script steps the run first; the opener then takes the
    /// run's script.
    fn step(&mut self, props: CoreProps) -> ScriptStep {
        let bracket = props.bracket_pair();
        let closes = match bracket {
            Some(pair) if !props.opens_bracket() => self.close(pair),
            _ => None,
        };
        let step = match (closes, props.scripts().single().map(kana_as_hiragana)) {
            (Some(script), _) => self.real(script),
            (None, Some(script)) if is_shared(script) => ScriptStep::Joins,
            (None, Some(script)) => self.real(script),
            (None, None) => self.shared(
                unicode::script_extensions(props),
                kana_as_hiragana(unicode::script(props)),
            ),
        };
        if let Some(pair) = bracket
            && props.opens_bracket()
        {
            self.open(pair);
        }
        step
    }

    /// Steps a character of one real script.
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

    /// Steps a character that may be any of `scripts`, whose own Script is
    /// `own`.
    fn shared(&mut self, scripts: &'static [ScriptId], own: ScriptId) -> ScriptStep {
        if scripts.is_empty() {
            // An empty set is no constraint. The table has none.
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

    /// Returns whether [`step`](Self::step) would join the cluster whose
    /// first character has `props` to the run and change nothing.
    ///
    /// True for a cluster with no paired bracket and one script, shared or
    /// the run's. The Latin-1 fast path passes most clusters by this test.
    #[inline]
    fn joins_quietly(&self, props: CoreProps) -> bool {
        props.bracket_pair().is_none()
            && props
                .scripts()
                .single()
                .is_some_and(|script| is_shared(script) || self.script == Some(script))
    }

    /// Returns whether the candidates still allow `script`.
    fn allows(&self, script: ScriptId) -> bool {
        self.candidates
            .iter()
            .take(64)
            .enumerate()
            .any(|(bit, &candidate)| candidate == script && self.allowed & (1 << bit) != 0)
    }

    /// Returns the run's script as it stands: decided, its first allowed
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

    /// Ends the run and starts one decided as `script` or undecided among
    /// `candidates`; returns the ended run's script.
    fn restart(&mut self, script: Option<ScriptId>, candidates: &'static [ScriptId]) -> ScriptStep {
        let ended = self.settled();
        self.script = script;
        self.candidates = candidates;
        self.allowed = all(candidates);
        self.undecided_from = self.depth;
        ScriptStep::Breaks(ended)
    }

    /// Decides the run as `script`, with the brackets it opened while
    /// undecided.
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

    /// Remembers an opening bracket with the run's script.
    fn open(&mut self, pair: BracketPairId) {
        if self.depth == MAX_BRACKETS {
            // Forgets the outermost.
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

    /// Closes a bracket and returns its opener's script, if it has an opener
    /// with a decided script. Closes the opener and everything opened since.
    fn close(&mut self, pair: BracketPairId) -> Option<ScriptId> {
        let open = self.brackets.get(..self.depth)?;
        let at = open.iter().rposition(|&(open, _)| open == pair)?;
        let (_, script) = *open.get(at)?;
        self.depth = at;
        self.undecided_from = self.undecided_from.min(at);
        (!is_shared(script)).then_some(script)
    }
}

/// Returns `script`, with Katakana taken as Hiragana.
///
/// OpenType tags both `kana`, so a run of both shapes as one and kerns
/// across them, as Blink's `GetScriptForOpenType` maps them. A
/// Script_Extensions set that holds Katakana holds Hiragana too.
fn kana_as_hiragana(script: ScriptId) -> ScriptId {
    if script == ScriptId::KATAKANA {
        ScriptId::HIRAGANA
    } else {
        script
    }
}

/// Returns whether `script` is shared by every script: Common, Inherited,
/// or Unknown.
pub(super) fn is_shared(script: ScriptId) -> bool {
    script == ScriptId::COMMON || script == ScriptId::INHERITED || script == ScriptId::UNKNOWN
}

/// Returns whether `script` is vertical-only per CSS Writing Modes 4:
/// Mongolian or Phags-pa. Its characters lie on their side in a vertical
/// line, even under `text-orientation: upright`.
pub(super) fn is_vertical_only(script: ScriptId) -> bool {
    matches!(&script.tag(), b"Mong" | b"Phag")
}

/// Returns the orientation of a cluster that follows a script run of
/// `script`: sideways in a vertical-only script, upright otherwise.
fn following(script: ScriptId) -> RunOrientation {
    if is_vertical_only(script) {
        RunOrientation::Sideways
    } else {
        RunOrientation::Upright
    }
}

/// Returns a bit for each of `scripts`, up to 64.
fn all(scripts: &[ScriptId]) -> u64 {
    match scripts.len() {
        0 => 0,
        n if n >= 64 => u64::MAX,
        n => (1 << n) - 1,
    }
}

/// Returns a bit for each of `candidates` that is also in `scripts`.
fn within(candidates: &[ScriptId], scripts: &[ScriptId]) -> u64 {
    candidates
        .iter()
        .take(64)
        .enumerate()
        .filter(|(_, candidate)| scripts.contains(candidate))
        .fold(0, |bits, (bit, _)| bits | (1 << bit))
}

/// How a cluster is set, as the runs read it: its orientation and its
/// combined unit.
///
/// Four bytes, which fit in a pending cluster's padding; every layout's
/// cluster loop copies one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Setting {
    pub(super) orientation: RunOrientation,
    /// The combined unit's number, or 0 outside one. Only neighbours are
    /// compared, so the numbers wrap around, and adjacent units never share
    /// one.
    pub(super) unit: u16,
    /// Whether it stands as its script run does: sideways in a vertical-only
    /// script, upright otherwise. `orientation` is upright until the script
    /// is known.
    pub(super) follows: bool,
}

impl Setting {
    /// The setting in a horizontal line, outside any unit.
    pub(super) const HORIZONTAL: Self = Self {
        orientation: RunOrientation::Horizontal,
        unit: 0,
        follows: false,
    };

    /// Returns the setting in a script run of `script`, where a following
    /// cluster takes the run's orientation.
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
    /// The open run's language, or `None` before the first run.
    language: Option<LanguageId>,
    /// The last cluster's setting; horizontal before the first cluster,
    /// which opens a run anyway.
    setting: Setting,
    /// The index of the current script run's first run in the list.
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
    /// A run ends where the language, script or setting changes, and around
    /// each combined unit. A unit is one run whatever scripts it holds,
    /// since it is set as one character; its script is the one its first
    /// script run resolves to. Scripts still resolve across it.
    ///
    /// A cluster that follows its script run is held upright while the run
    /// is undecided, and its runs turn when the run resolves. The orientation
    /// returned is then upright.
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
        // Inside a combined unit, only a paragraph start starts a run.
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

    /// Returns whether [`cluster`](Self::cluster) would change nothing for a
    /// horizontal cluster in `language` that does not open its paragraph.
    ///
    /// True if it joins the script run quietly and the open run has its
    /// language and setting. The Latin-1 fast path asks this first.
    #[inline]
    pub(super) fn joins(&self, props: CoreProps, language: LanguageId) -> bool {
        self.language == Some(language)
            && self.setting == Setting::HORIZONTAL
            && self.state.joins_quietly(props)
    }

    /// Settles the last script run at the text's end.
    pub(super) fn finish(&mut self, runs: &mut [PendingRun]) {
        settle(runs, self.from, self.state.settled());
    }
}

/// Sets `script` on the runs from index `from` on, and turns those that
/// follow it to its orientation.
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
/// A run takes its paragraph's level. In a mixed-level paragraph, runs also
/// split at each of `changes`, so every run has one level and shapes in one
/// direction. `end` is the text's end, where the last run ends.
pub(super) fn emit(
    runs: &[PendingRun],
    paragraphs: &Paragraphs,
    changes: &[(ClusterId, BidiLevel)],
    end: ClusterId,
    out: &mut ScriptRuns,
) {
    // One run per pending run, plus one per change.
    out.reserve(runs.len() + changes.len());
    let mut open = paragraphs.iter().peekable();
    let mut changes = changes.iter().copied().peekable();
    // The level set by the last change read, carried into later runs. A
    // mixed paragraph's first cluster has a change.
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
        // The level at its start is the last change at or before it, which
        // lies in its paragraph since the first cluster has one.
        while let Some((_, changed)) = changes.next_if(|&(at, _)| at <= run.start) {
            current = changed;
        }
        push(run.start, current);
        let end = runs.peek().map_or(end, |next| next.start);
        while let Some((at, changed)) = changes.next_if(|&(at, _)| at < end) {
            push(at, changed);
            current = changed;
        }
    }
}
