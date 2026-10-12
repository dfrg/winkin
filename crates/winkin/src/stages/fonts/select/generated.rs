//! Chooses the string and fonts of each generated text: a soft hyphen's
//! hyphen and a cut line's ellipsis.

use alloc::vec::Vec;

use fontwich::{FaceId, Presentation};
use icu_segmenter::GraphemeClusterSegmenter;
use parlance::{Language, Script};

use super::choice::{ResolvedRequest, choose};
use super::pick::{CandidateFit, GraphemeNeeds, ListOutcome, resolve, second_pass};
use super::{Input, MAX_STOPS, Selecting};
use crate::stages::analysis::ClusterClass;
use crate::stages::content::{FontRequest, FontRequestId, HyphenStringId, TextFactsId};
use crate::stages::fonts::context::CacheChange;
use crate::stages::fonts::lists::{CandidateId, CandidateList, ListKey, MatchRequest, NamedListId};
use crate::stages::fonts::{
    Generated, GeneratedFonts, GeneratedString, UsedFontId, coverage, features,
};
use crate::unicode::ScriptId;
use crate::{unicode, work};

/// Chooses the string and fonts of each generated text in `kinds` for text
/// with the facts `text`, into `out`, in order.
///
/// The string depends on what the request's primary font maps, as in
/// Blink's `ComputedStyle::HyphenString` and
/// `LineTruncator::ComputeEllipsisText`:
/// - hyphen: the text's `hyphenate-character`; for `auto`, U+2010 if the
///   primary maps it, else `-`;
/// - ellipsis: U+2026 if the primary maps it, else three full stops.
///
/// A string the primary maps whole, nearly every one, is one run in it.
/// Otherwise each grapheme ([`generated_graphemes`]) is set in the primary if it
/// covers it. If not, it takes the first candidate the text's walk would try
/// that covers it, then the best partial cover, then the primary. Pending
/// faces a grapheme wants go in the wanted faces.
///
/// Blink shapes a hyphen and an ellipsis in the style's `Font`, whose
/// fallback covers what the primary lacks (`HyphenResult::Shape`,
/// `LineTruncator`). This does the same through the text's candidate lists,
/// caches and used-font choices, applying cache changes as the walk does.
pub(crate) fn select_generated(
    input: &Input<'_>,
    text: TextFactsId,
    kinds: impl Iterator<Item = Generated>,
    sel: &mut Selecting<'_>,
    out: &mut GeneratedFonts,
) {
    let content = input.stage.content;
    let request_id = input.request(text);
    let request = content.facts.request(request_id);
    let primary = input.primary(request_id);
    let list = request.font.families;
    let language = input
        .default_language
        .fonts_language(content.languages.get(request.language));
    let primary_font = sel.cx.primary_font(list, request, language);
    let font = primary_font
        .as_ref()
        .and_then(|(family, index)| family.fonts().get(*index));
    let maps = |ch: char| font.is_some_and(|font| font.charset().contains(ch));
    for kind in kinds {
        let string = generated_string(kind, content.facts.text(text).hyphen, maps);
        let said = string.text(input.stage.lists.hyphen_strings());
        let start = out.next_run();
        if said.chars().all(maps) {
            out.push_run(start, said.len(), primary);
            out.push_text(text, kind, string, start);
            continue;
        }
        let generated = GeneratedRequest::new(input, request_id, request, list, language, said);
        for (end, grapheme) in generated_graphemes(said) {
            work::step();
            // As the text's walk asks: covered, and with every variation
            // sequence cluster matching asks for.
            let covered = font.is_some_and(|font| {
                coverage::covers(font, grapheme, &mut None)
                    && coverage::has_sequences(font, grapheme)
            });
            let font = match &generated {
                Some(generated) if !covered => generated.fallback(input, grapheme, sel),
                _ => primary,
            };
            out.push_run(start, end, font);
        }
        out.push_text(text, kind, string, start);
    }
}

/// What choosing a font for a grapheme of a generated text asks with.
///
/// It holds what the request asks of a font, and what its candidate list for
/// text is keyed by.
struct GeneratedRequest<'a> {
    resolved: ResolvedRequest<'a>,
    key: ListKey,
}

impl<'a> GeneratedRequest<'a> {
    /// Returns what choosing a font for the graphemes of `said` asks with, under
    /// `request` and its family list and language.
    ///
    /// Every request has its resolution. One that had none returns `None`,
    /// and its text is set in its primary alone.
    fn new(
        input: &Input<'_>,
        request_id: FontRequestId,
        request: &'a FontRequest,
        list: NamedListId,
        language: Language,
        said: &str,
    ) -> Option<Self> {
        let script = generated_script(said);
        let &resolution = input.resolutions.get(request_id)?;
        Some(Self {
            resolved: ResolvedRequest {
                request,
                request_id,
                resolution,
                script: features::opentype_script(script),
            },
            key: ListKey {
                list,
                script,
                language,
                request: MatchRequest::from(request),
                emoji: false,
            },
        })
    }

    /// The used font `grapheme`, which the primary font does not cover, is set
    /// in: the first candidate covering it, as the text's walk takes one.
    fn fallback(&self, input: &Input<'_>, grapheme: &str, sel: &mut Selecting<'_>) -> UsedFontId {
        for _ in 0..MAX_STOPS {
            match self.pick(input, grapheme, sel) {
                Ok(font) => return font.unwrap_or(self.resolved.resolution.primary),
                Err(change) => sel.cx.apply(change),
            }
        }
        debug_assert!(
            false,
            "a generated text's grapheme stopped {MAX_STOPS} times"
        );
        self.resolved.resolution.primary
    }

    /// Asks the candidate lists once for `grapheme`: the font, `None` where
    /// there is nothing to set it in but the primary, or the change to make
    /// before asking again.
    fn pick(
        &self,
        input: &Input<'_>,
        grapheme: &str,
        sel: &mut Selecting<'_>,
    ) -> Result<Option<UsedFontId>, CacheChange> {
        let Selecting {
            cx,
            scratch,
            used,
            wanted,
        } = sel;
        // Whether the grapheme holds a selector is read from it, which has no
        // clusters to say so.
        let variant = self.resolved.request.font.variant_emoji;
        let needs = GraphemeNeeds::new(generated_class(grapheme), grapheme, true, variant);
        let key = ListKey {
            emoji: needs.presentation.base() == Presentation::Emoji,
            ..self.key
        };
        let Some(list_id) = cx.span_list(key) else {
            return Ok(None);
        };
        let (lists, _, mut fonts) = cx.walk_caches();
        let Some(list) = lists.get(list_id) else {
            return Ok(None);
        };
        let pick = |needs: &GraphemeNeeds<'_>| pick_grapheme(list, wanted, needs);
        let found = resolve(lists, list, list_id, &needs, pick)?;
        let Some((at, held)) = found.and_then(|at| Some((at, list.get(at)?))) else {
            return Ok(None);
        };
        let Some(font) = held.font() else {
            return Ok(None);
        };
        let key = self.resolved.key(list_id, at);
        let choice = scratch.walk.choices.find_or_choose(key, || {
            choose(input, &self.resolved, font, held.family(), &mut fonts, used)
        })?;
        Ok(Some(choice.same))
    }
}

/// Returns the string a generated text of `kind` says.
///
/// `hyphen` is the text's `hyphenate-character`, if it names one, and `maps`
/// says whether the primary font maps a character.
fn generated_string(
    kind: Generated,
    hyphen: Option<HyphenStringId>,
    maps: impl Fn(char) -> bool,
) -> GeneratedString {
    match kind {
        Generated::Hyphen => match hyphen {
            Some(own) => GeneratedString::HyphenateCharacter(own),
            None if maps('\u{2010}') => GeneratedString::Hyphen,
            None => GeneratedString::HyphenMinus,
        },
        Generated::Ellipsis if maps('\u{2026}') => GeneratedString::Ellipsis,
        Generated::Ellipsis => GeneratedString::FullStops,
    }
}

/// Tries `list` for a grapheme of a generated text, as the walk's pick does but
/// with no memo, since the grapheme has no neighbours.
///
/// Finds the first candidate that accepts the presentation and covers the
/// grapheme with every variation sequence it asks for, else the first that
/// covers it. A pending face that wants the grapheme goes in `wanted`.
fn pick_grapheme(
    list: &CandidateList,
    wanted: &mut Vec<FaceId>,
    needs: &GraphemeNeeds<'_>,
) -> ListOutcome {
    let mut base: Option<CandidateId> = None;
    for (at, candidate) in list.iter() {
        work::step();
        match needs.fit(candidate, wanted, &mut None) {
            CandidateFit::Covers => return ListOutcome::Found(at),
            CandidateFit::CoversBase => {
                base.get_or_insert(at);
            }
            CandidateFit::Misses(_) | CandidateFit::Pending(_) | CandidateFit::Passed => {}
        }
    }
    second_pass(list, base)
}

/// Returns the class a grapheme of a generated text would have as a cluster:
/// emoji, pictograph (`Symbol`) or text.
///
/// A keycap makes a grapheme an emoji, as it does a cluster.
fn generated_class(grapheme: &str) -> ClusterClass {
    let Some(first) = grapheme.chars().next() else {
        return ClusterClass::Text;
    };
    let props = unicode::core_props(first);
    if props.is_emoji_presentation() || grapheme.contains('\u{20E3}') {
        ClusterClass::Emoji
    } else if props.is_extended_pictographic() {
        ClusterClass::Symbol
    } else {
        ClusterClass::Text
    }
}

/// The script a generated text's fonts are chosen for: that of its first
/// character with one of its own, or Common where none has, as Blink's
/// shaper resolves a string shaped alone.
fn generated_script(string: &str) -> Script {
    let script = string
        .chars()
        .map(|ch| unicode::script(unicode::core_props(ch)))
        .find(|&script| {
            script != ScriptId::COMMON
                && script != ScriptId::INHERITED
                && script != ScriptId::UNKNOWN
        })
        .unwrap_or(ScriptId::COMMON);
    Script::from_bytes(script.tag())
}

/// The graphemes of a generated text, each with where it ends: its grapheme
/// clusters, as UAX #29 finds them with ICU's segmenter, whose data is
/// compiled in, and which allocates nothing.
fn generated_graphemes(string: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    // The segmenter says 0 first, which ends no grapheme.
    GraphemeClusterSegmenter::new()
        .segment_str(string)
        .filter(|&end| end > 0)
        .map(move |end| {
            let grapheme = string.get(start..end).unwrap_or_default();
            start = end;
            (end, grapheme)
        })
}
