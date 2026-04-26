use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use std::fs;
use std::path::PathBuf;

use super::deps_dialog::show_dependencies_dialog;
use super::{SECONDARY_WINDOW_DEFAULT_HEIGHT, SECONDARY_WINDOW_DEFAULT_WIDTH};
use crate::config::{load_settings, save_settings};
use crate::prefix_tools::pick_and_run_in_prefix;
use crate::proton::resolve_proton_path;
use crate::tools::{gamemode_available, mangohud_available};
use crate::umu::get_umu_runtime_dir;

pub fn show_global_settings(parent: &adw::ApplicationWindow, overlay: &adw::ToastOverlay) {
    let settings = load_settings();

    let pref_window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .search_enabled(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .build();

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("emblem-system-symbolic")
        .build();

    let paths_group = adw::PreferencesGroup::builder()
        .title("Default Paths")
        .build();

    let prefix_row = adw::EntryRow::builder()
        .title("Default Prefix Path")
        .text(&settings.default_prefix_path)
        .build();

    // Build Proton dropdown – display basenames, store full paths via index
    let available_versions = settings.available_proton_versions.clone();
    let display_names: Vec<String> = available_versions
        .iter()
        .map(|p| {
            if p == "Default" {
                "Default".to_string()
            } else {
                PathBuf::from(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone())
            }
        })
        .collect();
    let proton_list = gtk4::StringList::new(&[]);
    for name in &display_names {
        proton_list.append(name);
    }

    let proton_row = adw::ComboRow::builder()
        .title("Default Proton")
        .model(&proton_list)
        .build();

    // Set selected index by matching full path
    if let Some(pos) = available_versions
        .iter()
        .position(|v| v == &settings.default_proton)
    {
        proton_row.set_selected(pos as u32);
    }

    paths_group.add(&prefix_row);
    paths_group.add(&proton_row);

    let tools_group = adw::PreferencesGroup::builder()
        .title("Tools")
        .description("Manage the default prefix inherited by games and groups.")
        .build();

    let manage_deps_btn = gtk4::Button::builder().label("Manage Dependencies").build();
    manage_deps_btn.set_margin_bottom(6);
    let run_prefix_btn = gtk4::Button::builder().label("Run in prefix").build();
    run_prefix_btn.set_margin_top(6);

    let parent_for_deps = parent.clone();
    let overlay_for_deps = overlay.clone();
    let prefix_row_for_deps = prefix_row.clone();
    let proton_row_for_deps = proton_row.clone();
    let available_versions_for_deps = available_versions.clone();
    manage_deps_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_deps.text().to_string();
        let proton_choice =
            if (proton_row_for_deps.selected() as usize) < available_versions_for_deps.len() {
                available_versions_for_deps[proton_row_for_deps.selected() as usize].clone()
            } else {
                "Default".to_string()
            };
        let proton = resolve_proton_path(&proton_choice).unwrap_or_default();
        show_dependencies_dialog(&parent_for_deps, &prefix, &proton, &overlay_for_deps);
    });

    let overlay_for_run = overlay.clone();
    let parent_for_run = parent.clone();
    let prefix_row_for_run = prefix_row.clone();
    let proton_row_for_run = proton_row.clone();
    let available_versions_for_run = available_versions.clone();
    run_prefix_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_run.text().to_string();
        let proton_choice =
            if (proton_row_for_run.selected() as usize) < available_versions_for_run.len() {
                available_versions_for_run[proton_row_for_run.selected() as usize].clone()
            } else {
                "Default".to_string()
            };
        let proton = resolve_proton_path(&proton_choice).unwrap_or_default();
        pick_and_run_in_prefix(&parent_for_run, &overlay_for_run, &prefix, &proton);
    });

    tools_group.add(&manage_deps_btn);
    tools_group.add(&run_prefix_btn);

    let environment_group = adw::PreferencesGroup::builder()
        .title("Global Environment")
        .build();

    let mangohud_row = adw::SwitchRow::builder()
        .title("MangoHud")
        .active(settings.global_mangohud)
        .visible(mangohud_available())
        .build();

    let gamemode_row = adw::SwitchRow::builder()
        .title("GameMode")
        .active(settings.global_gamemode)
        .visible(gamemode_available())
        .build();

    let wayland_row = adw::SwitchRow::builder()
        .title("Wayland")
        .active(settings.global_wayland)
        .build();

    let wow64_row = adw::SwitchRow::builder()
        .title("WoW64")
        .active(settings.global_wow64)
        .build();

    let ntsync_row = adw::SwitchRow::builder()
        .title("NTSync")
        .active(settings.global_ntsync)
        .build();

    environment_group.add(&mangohud_row);
    environment_group.add(&gamemode_row);
    environment_group.add(&wayland_row);
    environment_group.add(&wow64_row);
    environment_group.add(&ntsync_row);

    // ── Logging ────────────────────────────────────────────────────────────
    let logging_group = adw::PreferencesGroup::builder()
        .title("Logging")
        .description("Select which messages are printed to the terminal.")
        .build();

    let log_errors_row = adw::SwitchRow::builder()
        .title("Errors")
        .subtitle("Show error messages from leyen and launched processes")
        .active(settings.log_errors)
        .build();

    let log_warnings_row = adw::SwitchRow::builder()
        .title("Warnings")
        .subtitle("Show warning messages")
        .active(settings.log_warnings)
        .build();

    let log_operations_row = adw::SwitchRow::builder()
        .title("Operations")
        .subtitle("Show game launch and component installation activity")
        .active(settings.log_operations)
        .build();

    logging_group.add(&log_errors_row);
    logging_group.add(&log_warnings_row);
    logging_group.add(&log_operations_row);

    page.add(&paths_group);
    page.add(&tools_group);
    page.add(&environment_group);
    page.add(&logging_group);

    // ── Maintenance ────────────────────────────────────────────────────────
    let maintenance_group = adw::PreferencesGroup::builder()
        .title("Maintenance")
        .description("Use these actions to fix runtime issues.")
        .build();

    let runtime_repair_row = adw::ExpanderRow::builder()
        .title("Runtime Repair")
        .subtitle("Open repair actions for the local umu runtime.")
        .build();

    let reset_btn = gtk4::Button::builder()
        .label("Reset umu Runtime")
        .css_classes(["destructive-action"])
        .halign(gtk4::Align::Start)
        .build();
    let reset_row = adw::ActionRow::builder()
        .title("Reset umu Runtime")
        .subtitle("Delete the local Steam Linux Runtime so umu can download a fresh copy.")
        .activatable_widget(&reset_btn)
        .build();
    reset_row.add_suffix(&reset_btn);

    let pref_window_for_reset = pref_window.clone();
    let overlay_for_reset = overlay.clone();
    reset_btn.connect_clicked(move |_| {
        let confirm = gtk4::AlertDialog::builder()
            .message("Reset umu Runtime?")
            .detail(
                "This deletes the Steam Linux Runtime (steamrt3) directory. \
umu-launcher will re-download a clean copy the next time a dependency is installed.\n\n\
Use this to fix \"pressure-vessel-wrap\" errors during dependency installations.",
            )
            .buttons(vec!["Cancel".to_string(), "Reset".to_string()])
            .cancel_button(0)
            .default_button(0)
            .build();

        let overlay_clone = overlay_for_reset.clone();
        confirm.choose(
            Some(&pref_window_for_reset),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(1) = result {
                    let runtime_dir = get_umu_runtime_dir();
                    match fs::remove_dir_all(&runtime_dir) {
                        Ok(_) => {
                            overlay_clone.add_toast(adw::Toast::new(
                                "umu runtime reset. Re-run any dependency install to download a fresh copy.",
                            ));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            overlay_clone.add_toast(adw::Toast::new(
                                "umu runtime directory not found — nothing to reset.",
                            ));
                        }
                        Err(e) => {
                            overlay_clone.add_toast(adw::Toast::new(&format!(
                                "Failed to reset umu runtime: {}",
                                e
                            )));
                        }
                    }
                }
            },
        );
    });

    runtime_repair_row.add_row(&reset_row);
    maintenance_group.add(&runtime_repair_row);
    page.add(&maintenance_group);

    pref_window.add(&page);

    // Save settings when window is closed
    pref_window.connect_close_request(move |_| {
        let updated_settings = crate::models::GlobalSettings {
            default_prefix_path: prefix_row.text().to_string(),
            default_proton: if (proton_row.selected() as usize) < available_versions.len() {
                available_versions[proton_row.selected() as usize].clone()
            } else {
                "Default".to_string()
            },
            global_mangohud: mangohud_available() && mangohud_row.is_active(),
            global_gamemode: gamemode_available() && gamemode_row.is_active(),
            global_wayland: wayland_row.is_active(),
            global_wow64: wow64_row.is_active(),
            global_ntsync: ntsync_row.is_active(),
            available_proton_versions: available_versions.clone(),
            log_errors: log_errors_row.is_active(),
            log_warnings: log_warnings_row.is_active(),
            log_operations: log_operations_row.is_active(),
        };
        save_settings(&updated_settings);
        gtk4::glib::Propagation::Proceed
    });

    pref_window.present();
}
