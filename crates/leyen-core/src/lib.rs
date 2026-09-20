//! The Leyen engine (daemon side): game launch + systemd-scope lifecycle, the
//! single running-session monitor, the sandbox every Windows program runs in,
//! the log ring buffer, the dependency engine, runtime (umu/proton/winetricks)
//! installation, and the library/settings writer. No GTK — depends only on
//! `leyen-model` + tokio.

pub mod config;
pub mod deps;
pub mod launch;
pub mod logging;
pub mod prefix_tool;
pub mod runtime;
pub mod sandbox;
pub mod tools;
