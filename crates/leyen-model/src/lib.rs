//! Pure, dependency-light data layer shared by every Leyen binary.
//!
//! No tokio, no GTK. Serde models, i18n macros, filesystem path helpers, the
//! read-only library parse + in-memory mutation helpers, icon path helpers and
//! the dependency catalog/state read side.

pub mod deps;
pub mod i18n;
pub mod icons;
pub mod library;
pub mod models;
pub mod paths;

/// Application / D-Bus well-known name root and icon/app id.
pub const APP_ID: &str = "com.github.sachesi.leyen";
