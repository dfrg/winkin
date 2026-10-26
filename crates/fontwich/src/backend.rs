//! Platform fallback backends.
//!
//! A backend returns family names for a [`FallbackKey`].
//! [`Collection`](crate::Collection) resolves these names and caches the
//! installed families. Use [`FallbackOverride`](crate::FallbackOverride) to
//! supply a custom fallback order.

use crate::fallback::{BackendFacts, FallbackKey, Han};

#[cfg(fontwich_fontconfig)]
pub use crate::platform::Fontconfig;
#[cfg(all(target_vendor = "apple", feature = "system"))]
pub use crate::platform::MacOs;
#[cfg(any(windows, test))]
pub use crate::platform::Windows;
#[cfg(fontwich_android)]
pub use crate::platform::{Android, ConfigError, FamilyNames, SystemFonts};

/// A platform font fallback backend.
///
/// Returns family names in preference order. Backends do not cache results;
/// [`Collection`](crate::Collection) resolves installed families and caches
/// them by key.
#[derive(Debug)]
pub enum Backend {
    /// Windows fallback tables.
    #[cfg(any(windows, test))]
    Windows(Windows),
    /// Core Text fallback.
    #[cfg(all(target_vendor = "apple", feature = "system"))]
    Apple(MacOs),
    /// fontconfig fallback.
    #[cfg(fontwich_fontconfig)]
    Fontconfig(Fontconfig),
    /// Android configuration-based fallback.
    #[cfg(fontwich_android)]
    Android(Android),
}

impl Backend {
    /// Creates the current platform's fallback backend.
    ///
    /// Available on Windows, and with `system` on Apple, Linux, and BSD.
    /// Construct Android backends with `Android::from_fonts_xml`.
    #[cfg(any(
        windows,
        all(target_vendor = "apple", feature = "system"),
        fontwich_fontconfig
    ))]
    pub fn platform() -> Self {
        #[cfg(windows)]
        let backend = Self::Windows(Windows::new());
        #[cfg(all(target_vendor = "apple", feature = "system"))]
        let backend = Self::Apple(MacOs::new());
        #[cfg(fontwich_fontconfig)]
        let backend = Self::Fontconfig(Fontconfig::new());
        backend
    }

    /// Visits fallback family names in preference order.
    ///
    /// Names need not be installed. [`Collection`](crate::Collection) skips
    /// absent families and caches the resolved list. Core Text and
    /// fontconfig backends query the platform on each call.
    pub fn families(&self, key: &FallbackKey, mut name: impl FnMut(&str)) {
        let _ = (key, &mut name);
        match *self {
            #[cfg(any(windows, test))]
            Self::Windows(ref windows) => windows.families(key, &mut name),
            #[cfg(all(target_vendor = "apple", feature = "system"))]
            Self::Apple(ref apple) => apple.families(key, &mut name),
            #[cfg(fontwich_fontconfig)]
            Self::Fontconfig(ref fontconfig) => fontconfig.families(key, &mut name),
            #[cfg(fontwich_android)]
            Self::Android(ref android) => android.families(key, &mut name),
        }
    }

    /// Names the family the platform draws `c` with beyond what its keys
    /// name, in text of `han`'s tradition or of none.
    ///
    /// Calls `name` at most once. Only Windows answers, from DirectWrite's
    /// system fallback. A miss asks this after the Common key.
    pub(crate) fn character_family(&self, c: char, han: Option<Han>, mut name: impl FnMut(&str)) {
        let _ = (c, han, &mut name);
        match *self {
            #[cfg(any(windows, test))]
            Self::Windows(ref windows) => windows.character_family(c, han, &mut name),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// Returns what this backend's answers read of a request.
    pub(crate) fn facts(&self) -> BackendFacts {
        match *self {
            // The tables have no monospace rule for Arabic and Hebrew, so
            // the class is not read.
            #[cfg(any(windows, test))]
            Self::Windows(_) => BackendFacts::default(),
            #[cfg(all(target_vendor = "apple", feature = "system"))]
            Self::Apple(_) => BackendFacts::default(),
            #[cfg(fontwich_fontconfig)]
            Self::Fontconfig(_) => BackendFacts {
                per_language: true,
                reads_language: true,
                ..BackendFacts::default()
            },
            #[cfg(fontwich_android)]
            Self::Android(_) => BackendFacts {
                reads_serif: true,
                per_language: true,
                ..BackendFacts::default()
            },
        }
    }

    /// Whether a script's answer keeps only the families mapping the
    /// script's sample letters, checked by loading them.
    ///
    /// Android names one list per language, and only the fonts can say which
    /// of it belongs to a script.
    pub(crate) fn filters_scripts_by_loading(&self) -> bool {
        match *self {
            #[cfg(fontwich_android)]
            Self::Android(_) => true,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }
}
