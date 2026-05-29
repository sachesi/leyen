//! Localization (gettext) setup.
//!
//! The application picks up the user's system locale at startup (no in-app
//! language switching). Translatable strings are wrapped with the [`t!`] and
//! [`tn!`] macros and extracted into `po/leyen.pot`.

use std::path::PathBuf;

use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};

pub const TEXT_DOMAIN: &str = "leyen";

/// Resolves the directory containing compiled `.mo` catalogs.
///
/// Priority: the `LEYEN_LOCALEDIR` override, then `<prefix>/share/locale`
/// relative to the running executable (covers both Nix and FHS installs), then
/// the FHS default.
fn locale_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LEYEN_LOCALEDIR") {
        return PathBuf::from(dir);
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|bin| bin.parent())
    {
        let candidate = prefix.join("share").join("locale");
        if candidate.is_dir() {
            return candidate;
        }
    }

    PathBuf::from("/usr/share/locale")
}

/// Initializes gettext from the system locale. Call once, early in `main`.
pub fn init() {
    // Empty string means "use the environment" (LANG / LC_* / LC_ALL).
    setlocale(LocaleCategory::LcAll, "");

    let dir = locale_dir();
    if let Err(err) = bindtextdomain(TEXT_DOMAIN, &dir) {
        log::warn!("bindtextdomain failed for '{}': {err}", dir.display());
    }
    if let Err(err) = bind_textdomain_codeset(TEXT_DOMAIN, "UTF-8") {
        log::warn!("bind_textdomain_codeset failed: {err}");
    }
    if let Err(err) = textdomain(TEXT_DOMAIN) {
        log::warn!("textdomain failed: {err}");
    }
}

/// Translates a message via the active text domain.
pub fn gettext(msgid: &str) -> String {
    gettextrs::gettext(msgid)
}

/// Translates a singular/plural message based on `n`.
pub fn ngettext(singular: &str, plural: &str, n: u32) -> String {
    gettextrs::ngettext(singular, plural, n)
}

/// Wraps a translatable string literal: `t!("Launch Game")`.
#[macro_export]
macro_rules! t {
    ($msgid:expr) => {
        $crate::i18n::gettext($msgid)
    };
}

/// Plural-aware translation: `tn!("{} file", "{} files", n)`.
/// The returned string still contains the `{}` placeholder for the caller to
/// fill in (typically via `.replacen("{}", &n.to_string(), 1)`).
#[macro_export]
macro_rules! tn {
    ($singular:expr, $plural:expr, $n:expr) => {
        $crate::i18n::ngettext($singular, $plural, $n)
    };
}
