//! Localization (gettext) setup.
//!
//! The application picks up the user's system locale at startup (no in-app
//! language switching). Translatable strings go through [`gettext`] and
//! [`ngettext`]; `just pot` extracts them into `po/leyen.pot`.

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

/// Initializes gettext from the system locale.
///
/// # Safety
///
/// Call it first in `main`, before any thread is started: it sets the locale, which
/// reads the environment and changes state other threads may be reading.
pub unsafe fn init() {
    // Empty string means "use the environment" (LANG / LC_* / LC_ALL).
    // SAFETY: the caller has started no thread yet.
    unsafe { setlocale(LocaleCategory::LcAll, "") };

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
