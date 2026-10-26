//! The Windows backend: Blink's Windows fallback tables, with corrections.
//!
//! Children:
//! - `scripts`: the fonts for each script, from Chrome's `GetFallbackFamily`;
//! - `tail`: Chrome's catch-all lists, and the symbol, math, emoji and
//!   supplementary-plane Han fonts;
//! - `generics`: Chrome's generic families and Standard font;
//! - `mapchars`: DirectWrite's system fallback, recorded as a table;
//! - `directwrite`: the system layer, and `MapCharacters` asked at run time;
//! - `tests`: the tables against Chrome and Unicode, on any host.

#[cfg(all(windows, feature = "system"))]
mod directwrite;
mod generics;
mod mapchars;
mod scripts;
mod tail;
#[cfg(test)]
mod tests;

use parlance::Script;

#[cfg(all(windows, feature = "system"))]
pub(super) use directwrite::layer;

use crate::fallback::Presentation;
use crate::fallback::{FallbackKey, Han};
use crate::platform::windows::tail as chrome;
use crate::script as sc;

/// A Windows fallback backend.
///
/// Uses built-in script and fallback tables. Character misses use recorded
/// DirectWrite answers, or query DirectWrite when an answer is absent and
/// `system` is enabled.
///
/// Created by [`Backend::platform`](crate::backend::Backend::platform).
#[derive(Debug)]
pub struct Windows {}

impl Windows {
    // Off Windows only the tests make one.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// The families for `key`, best first:
    ///
    /// - a script: its fonts from Chrome's table, then Latin's, which every
    ///   script's text holds some of;
    /// - a Han tradition: its fonts, the traditions that travel with it,
    ///   Latin's, the supplementary-plane fonts, since Windows' CJK fonts stop
    ///   at the BMP, and Chrome's catch-all list for Han;
    /// - symbols and math: Segoe UI Symbol, and Cambria Math for math;
    /// - Common: Chrome's last resort and its catch-all list;
    /// - a generic, or the Standard font: Chrome's settings for its bucket;
    /// - emoji: the color font first, or for text presentation the
    ///   monochrome one.
    pub(crate) fn families(&self, key: &FallbackKey, name: &mut dyn FnMut(&str)) {
        let mut each = |names: &[&str]| names.iter().for_each(|family| name(family));
        if let Some(presentation) = key.presentation() {
            each(match presentation {
                Presentation::Text | Presentation::TextOnly => tail::EMOJI_TEXT,
                Presentation::Emoji | Presentation::EmojiOnly => tail::EMOJI,
            });
            return;
        }
        if let Some((generic, bucket)) = key.generic() {
            each(generics::TABLE.families(generic, bucket));
            return;
        }
        if let Some(bucket) = key.standard() {
            each(generics::TABLE.standard(bucket));
            return;
        }
        if key.is_common() {
            each(&[chrome::LAST_RESORT]);
            each(&chrome::COMMON_FONTS);
            return;
        }
        let Some(script) = key.script() else {
            return;
        };
        match script {
            sc::ZSYM => each(tail::SYMBOLS),
            sc::ZMTH => each(tail::MATH),
            sc::HANI => {
                let han = key.han().unwrap_or(Han::Hans);
                each(scripts::script_fonts(han.script()));
                for &companion in companions(han) {
                    each(scripts::script_fonts(companion));
                }
                each(scripts::script_fonts(sc::LATN));
                each(match han {
                    Han::Hant | Han::HantHK | Han::Kore => tail::PLANE_CJK_HANT,
                    Han::Hans | Han::Jpan => tail::PLANE_CJK_HANS,
                });
                each(&chrome::CJK_FONTS);
            }
            _ => {
                each(scripts::script_fonts(script));
                if script != sc::LATN {
                    each(scripts::script_fonts(sc::LATN));
                }
            }
        }
    }

    /// Names the one family DirectWrite's system fallback draws `c` with, in
    /// text of `han`'s tradition or of none.
    ///
    /// Reads the recorded table. Where it has no answer, a Windows build with
    /// `system` asks DirectWrite now.
    pub(crate) fn character_family(&self, c: char, han: Option<Han>, name: &mut dyn FnMut(&str)) {
        match mapchars::family(c, han) {
            Some(family) => name(family),
            #[cfg(all(windows, feature = "system"))]
            None => directwrite::map_character(c, mapchars::locale(han), name),
            #[cfg(not(all(windows, feature = "system")))]
            None => {}
        }
    }
}

/// Returns the scripts whose fonts follow `han`'s.
///
/// Japanese and Korean text borrow Chinese fonts for the Han their own
/// fonts lack. Traditional Chinese carries bopomofo.
fn companions(han: Han) -> &'static [Script] {
    match han {
        Han::Jpan | Han::Kore => &[sc::HANS, sc::HANT],
        Han::Hant | Han::HantHK => &[sc::BOPO, sc::HANS],
        Han::Hans => &[sc::HANT],
    }
}
