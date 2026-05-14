use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use std::fs;
use std::path::PathBuf;

use super::deps_dialog::show_dependencies_dialog;
use super::{SECONDARY_WINDOW_DEFAULT_HEIGHT, SECONDARY_WINDOW_DEFAULT_WIDTH};
use crate::prefix_tools::pick_and_run_in_prefix;
use crate::runtime::proton::resolve_proton_path;
use crate::runtime::umu::get_umu_runtime_dir;
use crate::tools::{gamemode_available, mangohud_available};
use gtk4::glib;

pub async fn show_global_settings(parent: &adw::ApplicationWindow) {
    let settings = crate::config::load_settings().await;

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .destroy_with_parent(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Global Settings", ""))
        .build();

    let close_btn = gtk4::Button::builder().label("Close").build();
    header.pack_start(&close_btn);

    let page = adw::PreferencesPage::builder().build();

    let paths_group = adw::PreferencesGroup::builder()
        .title("Default Paths")
        .build();

    let prefix_row = adw::EntryRow::builder()
        .title("Default Prefix Path")
        .text(&settings.default_prefix_path)
        .build();

    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for prefix folder")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let prefix_row_clone = prefix_row.clone();
    let dialog_clone = dialog.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(Some(&dialog_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

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
    let run_prefix_btn = gtk4::Button::builder()
        .label("Run in default prefix")
        .build();
    run_prefix_btn.set_margin_top(6);

    let overlay = adw::ToastOverlay::new();

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
        let parent = parent_for_deps.clone();
        let overlay = overlay_for_deps.clone();
        glib::spawn_future_local(async move {
            show_dependencies_dialog(&parent, &prefix, &proton, &overlay).await;
        });
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
        let parent = parent_for_run.clone();
        let overlay = overlay_for_run.clone();
        glib::spawn_future_local(async move {
            pick_and_run_in_prefix(&parent, &overlay, &prefix, &proton).await;
        });
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

    let hdr_row = adw::SwitchRow::builder()
        .title("HDR")
        .active(settings.global_hdr)
        .build();

    let proton_log_row = adw::SwitchRow::builder()
        .title("Proton Log")
        .active(settings.global_proton_log)
        .build();

    environment_group.add(&mangohud_row);
    environment_group.add(&gamemode_row);
    environment_group.add(&wayland_row);
    environment_group.add(&wow64_row);
    environment_group.add(&ntsync_row);
    environment_group.add(&hdr_row);
    environment_group.add(&proton_log_row);

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
        .subtitle("Show warning messages (e.g. game not found)")
        .active(settings.log_warnings)
        .build();

    let log_operations_row = adw::SwitchRow::builder()
        .title("Operations")
        .subtitle("Show info about background operations")
        .active(settings.log_operations)
        .build();

    logging_group.add(&log_errors_row);
    logging_group.add(&log_warnings_row);
    logging_group.add(&log_operations_row);

    // ── Maintenance ─────────────────────────────────────────────────────────
    let maintenance_group = adw::PreferencesGroup::builder()
        .title("Maintenance")
        .build();

    let runtime_repair_row = adw::ExpanderRow::builder()
        .title("Repair Runtime")
        .subtitle("Reset internal umu-launcher components if dependency installation fails.")
        .build();

    let reset_row = adw::ActionRow::builder()
        .title("Reset umu Runtime")
        .subtitle(
            "Deletes steamrt3 directory. It will be re-downloaded on next dependency install.",
        )
        .build();

    let reset_btn = gtk4::Button::builder()
        .label("Reset")
        .valign(gtk4::Align::Center)
        .css_classes(["destructive-action"])
        .build();
    reset_row.add_suffix(&reset_btn);

    let overlay_for_reset = overlay.clone();
    let dialog_for_reset = dialog.clone();
    reset_btn.connect_clicked(move |_| {
        let overlay_for_reset = overlay_for_reset.clone();
        let dialog_for_reset = dialog_for_reset.clone();
        glib::spawn_future_local(async move {
            let snapshots = crate::launch::running_games_snapshot().await;
            if !snapshots.is_empty() {
                overlay_for_reset.add_toast(adw::Toast::new(
                    "Cannot reset runtime while games are running. Close all games first.",
                ));
                return;
            }

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
                Some(&dialog_for_reset),
                gio::Cancellable::NONE,
                move |result| {
                    if let Ok(1) = result {
                        let overlay_clone = overlay_clone.clone();
                        let runtime_dir = get_umu_runtime_dir();
                        glib::spawn_future_local(async move {
                            let result = tokio::task::spawn_blocking(move || {
                                fs::remove_dir_all(&runtime_dir)
                            }).await.unwrap_or_else(|e| {
                                log::warn!("runtime reset task failed: {e}");
                                Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                            });

                            match result {
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
                        });
                    }
                },
            );
        });
    });

    runtime_repair_row.add_row(&reset_row);
    maintenance_group.add(&runtime_repair_row);

    page.add(&paths_group);
    page.add(&tools_group);
    page.add(&environment_group);
    page.add(&logging_group);
    page.add(&maintenance_group);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&page)
        .build();

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&scroll));

    overlay.set_child(Some(&toolbar_view));
    dialog.set_content(Some(&overlay));

    let dialog_clone = dialog.clone();
    close_btn.connect_clicked(move |_| dialog_clone.close());

    // Save settings when window is closed
    dialog.connect_close_request(move |_| {
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
            global_hdr: hdr_row.is_active(),
            global_proton_log: proton_log_row.is_active(),
            available_proton_versions: available_versions.clone(),
            log_errors: log_errors_row.is_active(),
            log_warnings: log_warnings_row.is_active(),
            log_operations: log_operations_row.is_active(),
        };
        glib::spawn_future_local(async move {
            crate::logging::apply_log_settings(&updated_settings);
            crate::config::save_settings(updated_settings).await;
        });
        gtk4::glib::Propagation::Proceed
    });

    dialog.present();
}
