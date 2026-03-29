use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;

use crate::config::load_settings;
use crate::deps::{
    DEP_CATALOG, DEP_CATEGORY_ORDER, add_installed_dep, get_dep_uninstall_steps,
    install_dep_async, read_installed_deps, remove_installed_dep, uninstall_dep_async,
};

fn dep_category_order(cat: &str) -> usize {
    DEP_CATEGORY_ORDER
        .iter()
        .position(|&c| c == cat)
        .unwrap_or(usize::MAX)
}

fn installed_subtitle(n: usize) -> String {
    match n {
        0 => "No components installed".to_string(),
        1 => "1 component installed".to_string(),
        n => format!("{} components installed", n),
    }
}

fn escape_dep_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn show_dependencies_dialog(
    parent: &adw::ApplicationWindow,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
) {
    let resolved_prefix = if !prefix_path.is_empty() {
        prefix_path.to_string()
    } else {
        let s = load_settings();
        if !s.default_prefix_path.is_empty() {
            s.default_prefix_path
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{}/.local/share/leyen/prefixes/default", home)
        }
    };

    let installed = read_installed_deps(&resolved_prefix);
    let installed_count = installed.len();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(640)
        .destroy_with_parent(true)
        .build();

    let subtitle = installed_subtitle(installed_count);

    let title_widget = adw::WindowTitle::new("Manage Dependencies", &subtitle);

    let header = adw::HeaderBar::builder()
        .title_widget(&title_widget)
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();

    let close_btn = gtk4::Button::builder().label("Close").build();
    header.pack_start(&close_btn);

    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search dependencies…")
        .margin_top(8)
        .margin_bottom(4)
        .margin_start(12)
        .margin_end(12)
        .build();

    let clamp = adw::Clamp::builder()
        .margin_top(4)
        .margin_bottom(8)
        .build();

    let dep_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    clamp.set_child(Some(&dep_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    content_box.append(&search_entry);
    content_box.append(&scroll);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));

    let mut entries: Vec<&crate::deps::DepCatalogEntry> = DEP_CATALOG.iter().collect();
    entries.sort_by(|a, b| {
        dep_category_order(a.category)
            .cmp(&dep_category_order(b.category))
            .then(a.name.cmp(b.name))
    });

    let mut categories: Vec<&str> = Vec::new();
    for e in &entries {
        if !categories.contains(&e.category) {
            categories.push(e.category);
        }
    }

    let mut groups: Vec<(adw::PreferencesGroup, Vec<(adw::ActionRow, &'static str)>)> = Vec::new();

    for cat in &categories {
        let group = adw::PreferencesGroup::builder().title(*cat).build();
        let mut rows_in_group: Vec<(adw::ActionRow, &'static str)> = Vec::new();

        for entry in entries.iter().filter(|e| e.category == *cat) {
            let dep_id = entry.id;
            let is_installed = installed.contains(dep_id);

            let row = adw::ActionRow::builder()
                .title(entry.name)
                .subtitle(&escape_dep_markup(entry.description))
                .build();

            let spinner = gtk4::Spinner::builder()
                .valign(gtk4::Align::Center)
                .visible(false)
                .build();

            let progress_label = gtk4::Label::builder()
                .label("")
                .css_classes(["caption", "dim-label"])
                .valign(gtk4::Align::Center)
                .visible(false)
                .max_width_chars(24)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();

            let install_btn = gtk4::Button::builder()
                .label("Install")
                .css_classes(["suggested-action"])
                .valign(gtk4::Align::Center)
                .visible(!is_installed)
                .build();

            let reinstall_btn = gtk4::Button::builder()
                .icon_name("view-refresh-symbolic")
                .tooltip_text("Reinstall")
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            let remove_btn = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove")
                .css_classes(["destructive-action"])
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            let badge = gtk4::Label::builder()
                .label("✓ Installed")
                .css_classes(["success", "caption"])
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            row.add_suffix(&badge);
            row.add_suffix(&spinner);
            row.add_suffix(&progress_label);
            row.add_suffix(&install_btn);
            row.add_suffix(&reinstall_btn);
            row.add_suffix(&remove_btn);

            // ── Install button ─────────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let spinner2 = spinner.clone();
                let progress_label2 = progress_label.clone();
                let row2 = row.clone();
                let badge2 = badge.clone();
                let title2 = title_widget.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();

                install_btn.connect_clicked(move |_| {
                    install_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);
                    row2.set_sensitive(false);

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let spinner3 = spinner2.clone();
                    let progress_label3 = progress_label2.clone();
                    let row3 = row2.clone();
                    let badge3 = badge2.clone();
                    let title3 = title2.clone();
                    let prefix3 = prefix2.clone();
                    let overlay3 = overlay2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, err: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        if success {
                            add_installed_dep(&prefix3, dep_id);
                            badge3.set_visible(true);
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let n = read_installed_deps(&prefix3).len();
                            title3.set_subtitle(&installed_subtitle(n));
                            overlay3.add_toast(adw::Toast::new(&format!(
                                "'{}' installed successfully.",
                                dep_id
                            )));
                        } else {
                            install_btn3.set_visible(true);
                            let msg = err.unwrap_or_else(|| "Installation failed.".to_string());
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    install_dep_async(
                        dep_id,
                        &prefix2,
                        &proton2,
                        &overlay2,
                        on_progress,
                        on_finish,
                    );
                });
            }

            // ── Reinstall button ───────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let spinner2 = spinner.clone();
                let progress_label2 = progress_label.clone();
                let row2 = row.clone();
                let badge2 = badge.clone();
                let title2 = title_widget.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();

                reinstall_btn.connect_clicked(move |_| {
                    reinstall_btn2.set_visible(false);
                    remove_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);
                    row2.set_sensitive(false);

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let spinner3 = spinner2.clone();
                    let progress_label3 = progress_label2.clone();
                    let row3 = row2.clone();
                    let badge3 = badge2.clone();
                    let title3 = title2.clone();
                    let prefix3 = prefix2.clone();
                    let overlay3 = overlay2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, err: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        if success {
                            add_installed_dep(&prefix3, dep_id);
                            badge3.set_visible(true);
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let n = read_installed_deps(&prefix3).len();
                            title3.set_subtitle(&installed_subtitle(n));
                            overlay3.add_toast(adw::Toast::new(&format!(
                                "'{}' reinstalled successfully.",
                                dep_id
                            )));
                        } else {
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let msg = err.unwrap_or_else(|| "Reinstall failed.".to_string());
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    install_dep_async(
                        dep_id,
                        &prefix2,
                        &proton2,
                        &overlay2,
                        on_progress,
                        on_finish,
                    );
                });
            }

            // ── Remove button ──────────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let spinner2 = spinner.clone();
                let progress_label2 = progress_label.clone();
                let row2 = row.clone();
                let badge2 = badge.clone();
                let title2 = title_widget.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();
                let dialog2 = dialog.clone();

                remove_btn.connect_clicked(move |_| {
                    let has_uninstall_steps = !get_dep_uninstall_steps(dep_id).is_empty();
                    let detail = if has_uninstall_steps {
                        "This will remove installed files and DLL overrides from the Wine prefix. \
                         This action cannot be undone."
                    } else {
                        "This removes the dependency from leyen's tracking. \
                         Installed files may remain in the Wine prefix — use \
                         Wine's Add/Remove Programs for a full uninstall."
                    };

                    let confirm = gtk4::AlertDialog::builder()
                        .message(&format!("Remove '{}'?", dep_id))
                        .detail(detail)
                        .buttons(vec!["Cancel".to_string(), "Remove".to_string()])
                        .cancel_button(0)
                        .default_button(0)
                        .build();

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let spinner3 = spinner2.clone();
                    let progress_label3 = progress_label2.clone();
                    let row3 = row2.clone();
                    let badge3 = badge2.clone();
                    let title3 = title2.clone();
                    let prefix3 = prefix2.clone();
                    let proton3 = proton2.clone();
                    let overlay3 = overlay2.clone();

                    confirm.choose(
                        Some(&dialog2),
                        gio::Cancellable::NONE,
                        move |result| {
                            if let Ok(1) = result {
                                if has_uninstall_steps {
                                    reinstall_btn3.set_visible(false);
                                    remove_btn3.set_visible(false);
                                    spinner3.set_visible(true);
                                    spinner3.start();
                                    progress_label3.set_visible(true);
                                    row3.set_sensitive(false);

                                    let install_btn4 = install_btn3.clone();
                                    let reinstall_btn4 = reinstall_btn3.clone();
                                    let remove_btn4 = remove_btn3.clone();
                                    let spinner4 = spinner3.clone();
                                    let progress_label4 = progress_label3.clone();
                                    let row4 = row3.clone();
                                    let badge4 = badge3.clone();
                                    let title4 = title3.clone();
                                    let prefix4 = prefix3.clone();
                                    let overlay4 = overlay3.clone();

                                    let progress_label_p = progress_label3.clone();
                                    let on_progress =
                                        move |_step: usize, _total: usize, desc: String| {
                                            progress_label_p.set_label(&desc);
                                        };

                                    let on_finish =
                                        move |success: bool, err: Option<String>| {
                                            spinner4.stop();
                                            spinner4.set_visible(false);
                                            progress_label4.set_visible(false);
                                            row4.set_sensitive(true);
                                            if success {
                                                remove_installed_dep(&prefix4, dep_id);
                                                badge4.set_visible(false);
                                                install_btn4.set_visible(true);
                                                reinstall_btn4.set_visible(false);
                                                remove_btn4.set_visible(false);
                                                let n = read_installed_deps(&prefix4).len();
                                                title4.set_subtitle(&installed_subtitle(n));
                                                overlay4.add_toast(adw::Toast::new(
                                                    &format!(
                                                        "'{}' uninstalled successfully.",
                                                        dep_id
                                                    ),
                                                ));
                                            } else {
                                                reinstall_btn4.set_visible(true);
                                                remove_btn4.set_visible(true);
                                                let msg = err.unwrap_or_else(|| {
                                                    "Uninstall failed.".to_string()
                                                });
                                                overlay4.add_toast(adw::Toast::new(&msg));
                                            }
                                        };

                                    uninstall_dep_async(
                                        dep_id,
                                        &prefix3,
                                        &proton3,
                                        &overlay3,
                                        on_progress,
                                        on_finish,
                                    );
                                } else {
                                    remove_installed_dep(&prefix3, dep_id);
                                    row3.set_sensitive(true);
                                    badge3.set_visible(false);
                                    install_btn3.set_visible(true);
                                    reinstall_btn3.set_visible(false);
                                    remove_btn3.set_visible(false);
                                    let n = read_installed_deps(&prefix3).len();
                                    title3.set_subtitle(&installed_subtitle(n));
                                    overlay3.add_toast(adw::Toast::new(&format!(
                                        "'{}' removed from tracking.",
                                        dep_id
                                    )));
                                }
                            }
                        },
                    );
                });
            }

            group.add(&row);
            rows_in_group.push((row, dep_id));
        }

        dep_box.append(&group);
        groups.push((group, rows_in_group));
    }

    // ── Search filtering ──────────────────────────────────────────────────
    let groups_for_search = groups.clone();
    search_entry.connect_search_changed(move |entry| {
        let query = entry.text().to_lowercase();
        for (group, rows) in &groups_for_search {
            let mut any_visible = false;
            for (row, dep_id) in rows {
                let visible = if query.is_empty() {
                    true
                } else {
                    let title = row.title().to_lowercase();
                    let subtitle = row
                        .subtitle()
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    title.contains(&query)
                        || subtitle.contains(&query)
                        || dep_id.contains(&query)
                };
                row.set_visible(visible);
                if visible {
                    any_visible = true;
                }
            }
            group.set_visible(query.is_empty() || any_visible);
        }
    });

    let dialog_close = dialog.clone();
    close_btn.connect_clicked(move |_| dialog_close.destroy());

    dialog.present();
}
