//! White space collapsing as one state machine.
//!
//! The writer classifies each character by `white-space-collapse` and passes
//! an [`Event`] to [`Collapser::step`], which returns the [`Effect`] the
//! writer applies. The collapser holds no text, so it is tested on events
//! alone.
//!
//! The rules follow Chrome's reading of CSS Text 3, section 4.1.1:
//!
//! - A run of collapsible white space becomes one space **where the run
//!   began**, even across box edges and ruby marks. The space is written
//!   only when content follows, so a run before the block end or a forced
//!   break writes nothing.
//! - **Atomic inlines end a run; box edges, floats, ruby marks and `<wbr>`
//!   do not** (Blink's `kOpaqueToCollapsing`). Box edges reach the
//!   collapser only for `white-space-trim`; the rest are never reported.
//! - **A wrap opportunity survives collapsing.** White space from a wrapping
//!   node joining a run from a non-wrapping one generates a break
//!   opportunity (Blink's `AppendGeneratedBreakOpportunity`).
//! - **A forced break removes collapsible white space on both sides.** Forced
//!   breaks are `<br>`, a kept segment break, and VT, FF, NEL, U+2028 and
//!   U+2029 (CSS Text 3, section 5.1).
//! - **A segment break** becomes a space unless U+200B is next to the run
//!   (Blink's `ShouldRemoveNewline`). Chrome compiles out the East Asian
//!   Width rule of CSS Text 3, section 4.1.2, and so does this crate.
//! - **Nothing collapsible is written at the block start** or after a forced
//!   break.
//! - **`white-space-trim`** acts on the pending run at box edges, which
//!   Chrome does not support. The writer handles the block's own
//!   `discard-inner`, since it also trims kept white space.
//! - **A combined unit** (`text-combine-upright: all`) collapses on its own,
//!   as Chrome's `LayoutTextCombine` is an inline block with its own text.
//!   Nothing collapsible is written at its start or end; the writer gives an
//!   empty unit a U+200B, as Blink's box stays one em wide. Outside, the unit
//!   is content.

use super::ItemId;
use crate::data::TextOffset;
use crate::style::WhiteSpaceTrim;

/// U+200B ZERO WIDTH SPACE.
pub(super) const ZWSP: char = '\u{200B}';

/// Where a run would begin if this white space starts one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Anchor {
    /// The end of the text so far, where the space goes.
    pub(super) at: TextOffset,
    /// The text item that takes the space: the node's open item, or the one
    /// the writer pushes if the run begins.
    pub(super) item: ItemId,
    /// The character before `at`, for the segment break rule.
    pub(super) before: Option<char>,
}

/// A run of collapsible white space that owes one space where it began.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Run {
    /// Where the space goes: the run's start.
    pub(super) at: TextOffset,
    /// The text item that takes the space, from the node the run began in.
    pub(super) item: ItemId,
    /// The character before the run.
    before: Option<char>,
    /// Whether the node the run began in wraps.
    wraps: bool,
    /// Whether the run holds a segment break.
    segment_break: bool,
    /// Whether a break opportunity has been generated for it.
    generated: bool,
}

/// An input to the collapser.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Event {
    /// A collapsible space, tab, lone carriage return, or collapsible
    /// segment break (`segment_break`). `wraps` is the node's wrapping.
    /// `anchor` is `None` when the writer has no room for the run's item,
    /// so no run can begin.
    Space {
        segment_break: bool,
        wraps: bool,
        anchor: Option<Anchor>,
    },
    /// Anything else written, starting with `first`: text, kept white space,
    /// or an atomic inline's U+FFFC.
    Content { first: char },
    /// A forced break: a kept segment break, `<br>`, or VT, FF, NEL, U+2028
    /// or U+2029.
    ForcedBreak,
    /// An inline box, ruby container or annotation opens.
    Open { trim: WhiteSpaceTrim },
    /// A box closes; `opened` is its opening item.
    Close {
        trim: WhiteSpaceTrim,
        opened: ItemId,
    },
    /// Whether the text that follows is combined
    /// (`text-combine-upright: all`). Sent at each text node and before an
    /// atomic inline (with `false`).
    Combined(bool),
    /// The block ends.
    End,
}

/// What the writer must do after an event.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Effect {
    /// Nothing.
    None,
    /// A run began: push the anchor's item if it is new.
    Began,
    /// Write the space `run` owes before the content that arrived.
    WriteSpace(Run),
    /// Write a generated break opportunity (U+200B) at the white space.
    GenerateBreak,
}

/// The collapser's state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum State {
    /// Collapsible white space is dropped: at the block start, after a
    /// forced break, and where `white-space-trim` discards it.
    Dropping,
    /// After content: white space begins a run.
    AfterContent,
    /// Inside a run that owes one space.
    Pending(Run),
}

/// The white space state machine.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Collapser {
    state: State,
    /// Inside a combined unit, which collapses its white space on its own.
    combining: bool,
}

impl Collapser {
    /// Creates a collapser at the block start, which drops white space.
    pub(super) const fn new() -> Self {
        Self {
            state: State::Dropping,
            combining: false,
        }
    }

    /// Whether the text arriving is in a combined unit.
    pub(super) fn is_combining(&self) -> bool {
        self.combining
    }

    /// Whether the text arriving is in a combined unit that has written
    /// nothing yet.
    pub(super) fn is_combining_empty(&self) -> bool {
        self.combining && self.state == State::Dropping
    }

    /// Applies `event` and returns what the writer must do.
    #[inline(always)]
    pub(super) fn step(&mut self, event: Event) -> Effect {
        // A combined unit ends at anything but its own text. Its trailing
        // run is dropped, and the unit is content to what follows.
        if self.combining && !matches!(event, Event::Space { .. } | Event::Content { .. }) {
            if event == Event::Combined(true) {
                return Effect::None;
            }
            self.combining = false;
            self.state = State::AfterContent;
        }
        match (self.state, event) {
            // A combined unit starts. It is content to the run before it,
            // and its own text starts dropping white space.
            (state, Event::Combined(true)) => {
                self.combining = true;
                self.state = State::Dropping;
                match state {
                    State::Pending(run) => Effect::WriteSpace(run),
                    State::Dropping | State::AfterContent => Effect::None,
                }
            }
            (_, Event::Combined(false)) => Effect::None,

            // White space.
            (State::Dropping, Event::Space { .. })
            | (State::AfterContent, Event::Space { anchor: None, .. }) => Effect::None,
            (
                State::AfterContent,
                Event::Space {
                    segment_break,
                    wraps,
                    anchor: Some(anchor),
                },
            ) => {
                self.state = State::Pending(Run {
                    at: anchor.at,
                    item: anchor.item,
                    before: anchor.before,
                    wraps,
                    segment_break,
                    generated: false,
                });
                Effect::Began
            }
            (
                State::Pending(mut run),
                Event::Space {
                    segment_break,
                    wraps,
                    ..
                },
            ) => {
                run.segment_break |= segment_break;
                let generate = wraps && !run.wraps && !run.generated;
                run.generated |= generate;
                self.state = State::Pending(run);
                if generate {
                    Effect::GenerateBreak
                } else {
                    Effect::None
                }
            }

            // Content ends a run. Its space is written unless the run holds
            // a segment break next to U+200B.
            (State::Pending(run), Event::Content { first }) => {
                self.state = State::AfterContent;
                let removed = run.segment_break && (run.before == Some(ZWSP) || first == ZWSP);
                if removed {
                    Effect::None
                } else {
                    Effect::WriteSpace(run)
                }
            }
            (_, Event::Content { .. }) => {
                self.state = State::AfterContent;
                Effect::None
            }

            // A forced break takes the white space on both sides.
            (_, Event::ForcedBreak) => {
                self.state = State::Dropping;
                Effect::None
            }

            // At an opening edge, `discard-before` drops the run before it
            // and `discard-inner` the white space after it. Otherwise an edge
            // is opaque.
            (State::Pending(_), Event::Open { trim }) if trim.discard_before => {
                self.state = if trim.discard_inner {
                    State::Dropping
                } else {
                    State::AfterContent
                };
                Effect::None
            }
            (State::AfterContent, Event::Open { trim }) if trim.discard_inner => {
                self.state = State::Dropping;
                Effect::None
            }
            (_, Event::Open { .. }) => Effect::None,

            // At a closing edge, `discard-inner` drops a run that began
            // inside and `discard-after` the white space after it.
            (State::Pending(run), Event::Close { trim, opened })
                if trim.discard_inner && run.item > opened =>
            {
                self.state = if trim.discard_after {
                    State::Dropping
                } else {
                    State::AfterContent
                };
                Effect::None
            }
            (State::AfterContent, Event::Close { trim, .. }) if trim.discard_after => {
                self.state = State::Dropping;
                Effect::None
            }
            (_, Event::Close { .. }) => Effect::None,

            // Nothing collapsible is written at the block end.
            (_, Event::End) => {
                self.state = State::Dropping;
                Effect::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Id;

    fn anchor(at: usize, item: usize) -> Option<Anchor> {
        Some(Anchor {
            at: TextOffset::new(at),
            item: ItemId::new(item),
            before: Some('a'),
        })
    }

    fn space(anchor: Option<Anchor>) -> Event {
        Event::Space {
            segment_break: false,
            wraps: true,
            anchor,
        }
    }

    #[test]
    fn white_space_at_the_start_is_dropped() {
        let mut c = Collapser::new();
        assert_eq!(c.step(space(anchor(0, 0))), Effect::None);
        assert_eq!(c.step(Event::Content { first: 'a' }), Effect::None);
    }

    #[test]
    fn a_run_owes_one_space_where_it_began() {
        let mut c = Collapser::new();
        c.step(Event::Content { first: 'a' });
        assert_eq!(c.step(space(anchor(1, 0))), Effect::Began);
        assert_eq!(c.step(space(anchor(1, 3))), Effect::None, "it joins");
        let Effect::WriteSpace(run) = c.step(Event::Content { first: 'b' }) else {
            panic!("the space is owed");
        };
        assert_eq!((run.at.get(), run.item.get()), (1, 0));
    }

    #[test]
    fn a_forced_break_takes_the_space_on_both_sides() {
        let mut c = Collapser::new();
        c.step(Event::Content { first: 'a' });
        c.step(space(anchor(1, 0)));
        assert_eq!(c.step(Event::ForcedBreak), Effect::None);
        assert_eq!(c.step(space(anchor(2, 1))), Effect::None);
        assert_eq!(c.step(Event::Content { first: 'b' }), Effect::None);
    }

    #[test]
    fn white_space_from_a_wrapping_node_keeps_its_opportunity() {
        let mut c = Collapser::new();
        c.step(Event::Content { first: 'a' });
        let nowrap = Event::Space {
            segment_break: false,
            wraps: false,
            anchor: anchor(1, 0),
        };
        assert_eq!(c.step(nowrap), Effect::Began);
        assert_eq!(c.step(space(None)), Effect::GenerateBreak);
        assert_eq!(c.step(space(None)), Effect::None, "once per run");
    }

    #[test]
    fn a_combined_unit_drops_its_own_leading_and_trailing_white_space() {
        let mut c = Collapser::new();
        c.step(Event::Content { first: 'a' });
        assert_eq!(c.step(space(anchor(1, 0))), Effect::Began);
        // The run before the unit owes its space, as before an atomic inline.
        let Effect::WriteSpace(run) = c.step(Event::Combined(true)) else {
            panic!("the space before the unit is owed");
        };
        assert_eq!(run.at.get(), 1);
        assert_eq!(c.step(space(anchor(2, 1))), Effect::None, "its start");
        assert_eq!(c.step(Event::Content { first: '1' }), Effect::None);
        assert_eq!(c.step(space(anchor(3, 1))), Effect::Began, "inside");
        assert!(matches!(
            c.step(Event::Content { first: '2' }),
            Effect::WriteSpace(_)
        ));
        c.step(space(anchor(4, 1)));
        assert_eq!(c.step(Event::Combined(true)), Effect::None, "a sibling");
        assert_eq!(c.step(Event::Combined(false)), Effect::None, "its end");
        assert_eq!(c.step(Event::Content { first: 'b' }), Effect::None);
    }

    #[test]
    fn a_segment_break_beside_a_zero_width_space_is_removed() {
        let mut c = Collapser::new();
        c.step(Event::Content { first: 'a' });
        c.step(Event::Space {
            segment_break: true,
            wraps: true,
            anchor: anchor(1, 0),
        });
        assert_eq!(c.step(Event::Content { first: ZWSP }), Effect::None);
    }
}
