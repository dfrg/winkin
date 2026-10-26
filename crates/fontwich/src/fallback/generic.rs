//! The CSS generic families, and the browser font settings they read.
//!
//! A `Generic(family, bucket)` key resolves to the first
//! installed family of an ordered list. Each platform's backend holds its
//! lists in a [`GenericTable`]. This module holds what every platform
//! shares: which generics exist, the setting each reads, and how a bucket
//! with no value of its own takes Common's.
//!
//! Chrome reads a generic from its font setting for the generic and the
//! content locale's script (`FontSelector::FamilyNameFromSettings`,
//! `font_selector.cc:71–90`), whose defaults are fixed per platform build:
//!
//! - a value is a default only where `kFontDefaults` registers it
//!   (`prefs_tab_helper.cc:147–222`); every other script's setting is empty
//!   and takes Common's (`GenericFontFamilySettings::GenericFontFamilyForScript`,
//!   `generic_font_family_settings.cc:81–108`);
//! - a value starting with `,` is a list, resolved to its first installed
//!   family (`gfx::FontList::FirstAvailableOrFirst`,
//!   `prefs_tab_helper.cc:447–450`), so the whole list is kept in order.
//!
//! Separately, Chrome tries the content locale's **Standard** font after
//! every `font-family` list, before system fallback
//! (`font_fallback_list.cc:177–187`): [`GenericTable::standard`].
//!
//! **Not modelled: the browser's UI language.** Chrome drops the default
//! for the script of its UI locale (`prefs_tab_helper.cc:430–441`), and the
//! Common values are translated per UI locale
//! (`platform_locale_settings/locale_settings_*_<lang>.xtb`). The tables
//! are an English UI's. A host whose users run another UI language
//! supplies that difference itself.

// A build with no backend reads no table: only the key's folding is left.
#![cfg_attr(not(test), allow(dead_code))]

use parlance::GenericFamily;

use crate::fallback::key::GenericBucket;

/// Collapses the `ui-*` aliases onto the family they follow, so a key and a
/// backend only have to carry the distinct cases.
///
/// `ui-rounded` folds to sans-serif: no platform we target has a rounded font
/// as a system default, and sans is the closest honest answer.
pub(crate) fn normalize(generic: GenericFamily) -> GenericFamily {
    match generic {
        GenericFamily::UiSerif => GenericFamily::Serif,
        GenericFamily::UiSansSerif | GenericFamily::UiRounded => GenericFamily::SansSerif,
        GenericFamily::UiMonospace => GenericFamily::Monospace,
        other => other,
    }
}

/// One of Chrome's seven font settings (`prefs::kWebKit*FontFamily`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Setting {
    Standard,
    Fixed,
    Serif,
    SansSerif,
    Cursive,
    Fantasy,
    Math,
}

/// Returns the setting `family` reads, if any.
///
/// `ui-*` fold as the key does. `system-ui` reads no setting: the platform
/// answers it. Chrome reads none of `ui-*`, `emoji` and `fangsong` as
/// generics: to it they are family names no font has, so `emoji` and
/// `fangsong` read nothing (the key sends `emoji` to the emoji fonts before
/// it gets here).
pub(crate) fn setting(family: GenericFamily) -> Option<Setting> {
    use GenericFamily::*;
    match normalize(family) {
        Serif => Some(Setting::Serif),
        SansSerif => Some(Setting::SansSerif),
        Monospace => Some(Setting::Fixed),
        Cursive => Some(Setting::Cursive),
        Fantasy => Some(Setting::Fantasy),
        Math => Some(Setting::Math),
        SystemUi | Emoji | FangSong => None,
        // `normalize` has folded these.
        UiSerif | UiSansSerif | UiMonospace | UiRounded => None,
    }
}

/// A platform's families for the generics: its defaults for Chrome's
/// settings, and its system UI font.
#[derive(Copy, Clone, Debug)]
pub(crate) struct GenericTable {
    /// The families a setting names in a bucket, best first, where the
    /// platform registers a default. `None` means the bucket takes Common's.
    pub(crate) defaults: fn(Setting, GenericBucket) -> Option<&'static [&'static str]>,
    /// The system UI font Chrome asks the platform for: `system-ui` is no
    /// setting, and `FontCache::SystemFontPlatformData` answers it.
    pub(crate) system_ui: &'static [&'static str],
}

impl GenericTable {
    /// Returns the families `family` resolves to in `bucket`, best first.
    ///
    /// The first installed one applies, and none means the generic names no
    /// font. Names match case-insensitively (`NSimsun` is `NSimSun`).
    pub(crate) fn families(
        &self,
        family: GenericFamily,
        bucket: GenericBucket,
    ) -> &'static [&'static str] {
        match setting(family) {
            Some(setting) => self.setting_families(setting, bucket),
            None if family == GenericFamily::SystemUi => self.system_ui,
            None => &[],
        }
    }

    /// Returns the Standard font's families in `bucket`, best first, tried
    /// after every `font-family` list; the first installed one applies.
    pub(crate) fn standard(&self, bucket: GenericBucket) -> &'static [&'static str] {
        self.setting_families(Setting::Standard, bucket)
    }

    /// Returns `setting`'s families in `bucket`, or Common's where the
    /// bucket has no default.
    fn setting_families(&self, setting: Setting, bucket: GenericBucket) -> &'static [&'static str] {
        (self.defaults)(setting, bucket)
            .or_else(|| (self.defaults)(setting, GenericBucket::Common))
            .unwrap_or(&[])
    }
}
