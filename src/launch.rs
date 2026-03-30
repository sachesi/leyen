use libadwaita as adw;

use gtk4::gio;

use crate::config::load_settings;
use crate::logging::leyen_log;
use crate::models::Game;
use crate::proton::resolve_proton_path;
use crate::umu::{get_umu_run_path, is_umu_run_available, UMU_DOWNLOADING};

pub fn launch_game(game: &Game, overlay: &adw::ToastOverlay) {
    // Block launch while umu-launcher is being downloaded.
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    // Block launch if umu-run is simply not available.
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let settings = load_settings();
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);

    // Wine prefix
    if !game.prefix_path.is_empty() {
        launcher.setenv("WINEPREFIX", &game.prefix_path, true);
    }

    // Game ID for umu-run
    if !game.game_id.is_empty() {
        launcher.setenv("GAMEID", &game.game_id, true);
    }

    // Proton path (resolve backward-compat names to full paths)
    if let Some(proton_path) = resolve_proton_path(&game.proton) {
        launcher.setenv("PROTONPATH", &proton_path, true);
    }

    // MangoHud – per-game flag OR global setting
    if game.force_mangohud || settings.global_mangohud {
        launcher.setenv("MANGOHUD", "1", true);
    }

    // Wayland: per-game override OR global setting
    launcher.setenv(
        "PROTON_ENABLE_WAYLAND",
        if game.game_wayland || settings.global_wayland {
            "1"
        } else {
            "0"
        },
        true,
    );

    // WoW64: per-game override OR global setting
    launcher.setenv(
        "PROTON_USE_WOW64",
        if game.game_wow64 || settings.global_wow64 {
            "1"
        } else {
            "0"
        },
        true,
    );

    // NTSync: per-game override OR global setting
    let ntsync_val = if game.game_ntsync || settings.global_ntsync {
        "1"
    } else {
        "0"
    };
    launcher.setenv("PROTON_USE_NTSYNC", ntsync_val, true);
    launcher.setenv("WINENTSYNC", ntsync_val, true);

    // Build the argument list, honouring Steam-style %command% substitution.
    // If the launch-args field contains "%command%", everything before it is
    // examined token by token:
    //   • KEY=VALUE tokens are applied as environment variables
    //   • other tokens (e.g. "gamemoderun") are prepended as command wrappers
    // Everything after "%command%" is appended after the executable path.
    // Without "%command%", extra args are appended after the executable path as before.
    let umu = get_umu_run_path();
    let mut cmd_args: Vec<String> = Vec::new();

    if game.launch_args.contains("%command%") {
        let parts: Vec<&str> = game.launch_args.splitn(2, "%command%").collect();
        let postfix: Vec<String> = parts
            .get(1)
            .unwrap_or(&"")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        // Classify prefix tokens as env vars or command wrappers
        let mut cmd_wrappers: Vec<String> = Vec::new();
        for token in parts[0].split_whitespace() {
            // A token is an env var if it looks like KEY=VALUE (no spaces, contains '=')
            if let Some(eq_pos) = token.find('=') {
                let key = &token[..eq_pos];
                let val = &token[eq_pos + 1..];
                // Only treat as env var if the key is a valid identifier (no special chars)
                if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    launcher.setenv(key, val, true);
                    continue;
                }
            }
            cmd_wrappers.push(token.to_string());
        }

        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.extend(cmd_wrappers);
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        cmd_args.extend(postfix);
    } else {
        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        if !game.launch_args.is_empty() {
            cmd_args.extend(game.launch_args.split_whitespace().map(|s| s.to_string()));
        }
    }

    let os_args: Vec<&std::ffi::OsStr> = cmd_args.iter().map(std::ffi::OsStr::new).collect();

    leyen_log("INFO ", &format!(
        "Launching '{}' | exe: {} | prefix: {} | proton: {}",
        game.title,
        game.exe_path,
        game.prefix_path,
        game.proton,
    ));

    match launcher.spawn(&os_args) {
        Ok(_) => {
            overlay.add_toast(adw::Toast::new(&format!("Launching {}...", game.title)));
        }
        Err(e) => {
            leyen_log("ERROR", &format!("Failed to launch '{}': {}", game.title, e));
            overlay.add_toast(adw::Toast::new(&format!("Failed to launch: {}", e)));
        }
    }
}
