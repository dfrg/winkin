//! Stored runs from text analysis.

use parlance::Script;

use crate::data::{Run, Runs};
use crate::stages::content::LanguageId;
use crate::style::BaseDirection;

use super::{ClusterId, ScriptRunId};

/// A resolved Unicode bidi level: even for left to right, odd for right to
/// left.
///
/// Higher levels are deeper embeddings. Returned by
/// [`Line::level`](crate::Line::level),
/// [`TextRun::level`](crate::TextRun::level),
/// [`BoxFragment::level`](crate::BoxFragment::level) and
/// [`Atomic::level`](crate::Atomic::level).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BidiLevel(u8);

impl BidiLevel {
    /// Level 0, the level of a left-to-right paragraph, and the default where
    /// nothing resolved a level.
    pub(crate) const LTR: Self = Self(0);

    /// Wraps a resolved level.
    pub(crate) const fn new(level: u8) -> Self {
        Self(level)
    }

    /// Returns the paragraph level `direction` sets: 0 for left to right, 1
    /// for right to left, and `None` for `auto`.
    ///
    /// Under `auto`, each paragraph's first strong character decides (UAX #9
    /// rules P2 and P3). `BaseDirection` is a foreign type, so this lives
    /// here.
    pub(crate) const fn from_direction(direction: BaseDirection) -> Option<Self> {
        match direction {
            BaseDirection::Ltr => Some(Self(0)),
            BaseDirection::Rtl => Some(Self(1)),
            BaseDirection::Auto => None,
        }
    }

    /// Returns the numeric level, from 0 to 126.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Returns `true` if the level is odd (right to left).
    pub const fn is_rtl(self) -> bool {
        self.0 % 2 == 1
    }
}

/// A maximal run of clusters with one script, language, level and
/// orientation, in 12 bytes.
///
/// Runs tile the clusters and never cross a paragraph. Each ends where the
/// next starts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScriptRun {
    pub(crate) start: ClusterId,
    /// The script, resolved by UAX #24 with Script_Extensions and paired
    /// brackets (see `scripts`).
    pub(crate) script: Script,
    pub(crate) language: LanguageId,
    pub(crate) level: BidiLevel,
    pub(crate) orientation: RunOrientation,
}

impl Run for ScriptRun {
    type Position = ClusterId;

    #[inline]
    fn start(&self) -> ClusterId {
        self.start
    }
}

/// The script runs, in text order, tiling the clusters.
///
/// A stage walking the clusters forward keeps a cursor into them rather than
/// looking each cluster up.
pub(crate) type ScriptRuns = Runs<ScriptRunId, ScriptRun>;

/// The orientation of a text run's glyphs.
///
/// Returned by [`TextRun::orientation`](crate::layout::TextRun::orientation).
/// Positions are in the line frame in every writing mode. A painter maps the
/// frame to the page, rotates sideways glyphs with the line, and leaves
/// upright and combined glyphs upright at their origins.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum RunOrientation {
    /// Text in a horizontal line (`horizontal-tb`).
    Horizontal,
    /// Upright text in a vertical line, shaped along the line.
    ///
    /// Each glyph is centered across the line. Its origin lies half the glyph
    /// width under the center of the line, and the height of its vertical
    /// origin along the line.
    ///
    /// Applies to ideographs and kana under `text-orientation: mixed`, and
    /// under `text-orientation: upright` to every character outside the
    /// vertical scripts.
    Upright,
    /// Sideways text in a vertical line, shaped as horizontal text and
    /// rotated with the line.
    ///
    /// Applies under `text-orientation: mixed` to characters not set
    /// upright, such as Latin, and to every character under
    /// `text-orientation: sideways` or in the `sideways-rl` and `sideways-lr`
    /// writing modes.
    Sideways,
    /// Combined text (`text-combine-upright`).
    ///
    /// Shaped as horizontal text, fitted to one em, and set upright in one em
    /// of the line, centered across it.
    Combined,
}
