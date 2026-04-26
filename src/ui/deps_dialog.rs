use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use std::cell::Cell;
use std::rc::Rc;

use crate::config::load_settings;
use crate::deps::{
    DEP_CATEGORY_ORDER, DEP_PROFILES, find_installed_dependents, get_dep_profile,
    get_installed_dep, install_dep_async, read_installed_deps, read_prefix_dep_state,
    uninstall_dep_async,
};

use super::{SECONDARY_WINDOW_DEFAULT_HEIGHT, SECONDARY_WINDOW_DEFAULT_WIDTH};

#[derive(Clone)]
struct DepRowHandle {
    dep_id: &'static str,
    install_btn: gtk4::Button,
    reinstall_btn: gtk4::Button,
    remove_btn: gtk4::Button,
    badge: gtk4::Label,
}

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

fn dependent_names(state: &crate::deps::PrefixDependencyState, dep_id: &str) -> Vec<String> {
    find_installed_dependents(state, dep_id)
        .into_iter()
        .map(|dependent_id| {
            get_dep_profile(&dependent_id)
                .map(|profile| profile.name.to_string())
                .unwrap_or(dependent_id)
        })
        .collect()
}

fn sync_dep_row(
    handle: &DepRowHandle,
    installed: &std::collections::BTreeSet<String>,
    dependents: &[String],
) {
    let is_installed = installed.contains(handle.dep_id);
    handle.badge.set_visible(is_installed);
    handle.install_btn.set_visible(!is_installed);
    handle.reinstall_btn.set_visible(is_installed);
    handle.remove_btn.set_visible(is_installed);
    let can_remove = is_installed && dependents.is_empty();
    handle.remove_btn.set_sensitive(can_remove);
    if can_remove {
        handle
            .remove_btn
            .set_tooltip_text(Some("Remove this managed dependency"));
    } else if is_installed {
        handle
            .remove_btn
            .set_tooltip_text(Some(&format!("Required by: {}", dependents.join(", "))));
    } else {
        handle.remove_btn.set_tooltip_text(None);
    }
}

fn refresh_dep_rows(
    prefix_path: &str,
    title_widget: &adw::WindowTitle,
    handles: &[DepRowHandle],
) -> std::collections::BTreeSet<String> {
    let state = read_prefix_dep_state(prefix_path);
    let installed = state
        .installed
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    title_widget.set_subtitle(&installed_subtitle(installed.len()));
    for handle in handles {
        let dependents = dependent_names(&state, handle.dep_id);
        sync_dep_row(handle, &installed, &dependents);
    }
    installed
}

fn set_dialog_busy(
    busy: bool,
    close_btn: &gtk4::Button,
    search_entry: &gtk4::SearchEntry,
    handles: &[DepRowHandle],
) {
    close_btn.set_sensitive(!busy);
    search_entry.set_sensitive(!busy);
    for handle in handles {
        handle.install_btn.set_sensitive(!busy);
        handle.reinstall_btn.set_sensitive(!busy);
        handle.remove_btn.set_sensitive(!busy);
    }
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

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .destroy_with_parent(true)
        .build();

    let subtitle = installed_subtitle(installed.len());

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

    let page = adw::PreferencesPage::builder().build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&page)
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
    let dialog_busy = Rc::new(Cell::new(false));
    {
        let dialog_busy = dialog_busy.clone();
        let overlay = overlay.clone();
        dialog.connect_close_request(move |_| {
            if dialog_busy.get() {
                overlay.add_toast(adw::Toast::new(
                    "Wait for the dependency operation to finish.",
                ));
                gtk4::glib::Propagation::Stop
            } else {
                gtk4::glib::Propagation::Proceed
            }
        });
    }

    let mut entries: Vec<&crate::deps::DepProfile> = DEP_PROFILES.iter().collect();
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
    let row_handles = std::rc::Rc::new(std::cell::RefCell::new(Vec::<DepRowHandle>::new()));

    for cat in &categories {
        let group = adw::PreferencesGroup::builder().title(*cat).build();
        let mut rows_in_group: Vec<(adw::ActionRow, &'static str)> = Vec::new();

        for entry in entries.iter().filter(|e| e.category == *cat) {
            let dep_id = entry.id;
            let is_installed = installed.contains(dep_id);

            let row = adw::ActionRow::builder()
                .title(entry.name)
                .subtitle(escape_dep_markup(entry.description))
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
            row_handles.borrow_mut().push(DepRowHandle {
                dep_id,
                install_btn: install_btn.clone(),
                reinstall_btn: reinstall_btn.clone(),
                remove_btn: remove_btn.clone(),
                badge: badge.clone(),
            });

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
                let row_handles2 = row_handles.clone();
                let close_btn2 = close_btn.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();

                install_btn.connect_clicked(move |_| {
                    dialog_busy2.set(true);
                    set_dialog_busy(true, &close_btn2, &search_entry2, &row_handles2.borrow());
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
                    let row_handles3 = row_handles2.clone();
                    let close_btn3 = close_btn2.clone();
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, note_or_error: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        refresh_dep_rows(&prefix3, &title3, &row_handles3.borrow());
                        dialog_busy3.set(false);
                        set_dialog_busy(false, &close_btn3, &search_entry3, &row_handles3.borrow());
                        if success {
                            badge3.set_visible(true);
                            let message = note_or_error
                                .map(|note| {
                                    format!("'{}' installed successfully. {}", dep_id, note)
                                })
                                .unwrap_or_else(|| format!("'{}' installed successfully.", dep_id));
                            overlay3.add_toast(adw::Toast::new(&message));
                        } else {
                            install_btn3.set_visible(true);
                            reinstall_btn3.set_visible(false);
                            remove_btn3.set_visible(false);
                            let msg =
                                note_or_error.unwrap_or_else(|| "Installation failed.".to_string());
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
                let row_handles2 = row_handles.clone();
                let close_btn2 = close_btn.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();

                reinstall_btn.connect_clicked(move |_| {
                    dialog_busy2.set(true);
                    set_dialog_busy(true, &close_btn2, &search_entry2, &row_handles2.borrow());
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
                    let row_handles3 = row_handles2.clone();
                    let close_btn3 = close_btn2.clone();
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, note_or_error: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        refresh_dep_rows(&prefix3, &title3, &row_handles3.borrow());
                        dialog_busy3.set(false);
                        set_dialog_busy(false, &close_btn3, &search_entry3, &row_handles3.borrow());
                        if success {
                            badge3.set_visible(true);
                            let message = note_or_error
                                .map(|note| {
                                    format!("'{}' reinstalled successfully. {}", dep_id, note)
                                })
                                .unwrap_or_else(|| {
                                    format!("'{}' reinstalled successfully.", dep_id)
                                });
                            overlay3.add_toast(adw::Toast::new(&message));
                        } else {
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let msg =
                                note_or_error.unwrap_or_else(|| "Reinstall failed.".to_string());
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
                let row_handles2 = row_handles.clone();
                let close_btn2 = close_btn.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();

                remove_btn.connect_clicked(move |_| {
                    let detail = get_installed_dep(&prefix2, dep_id)
                        .map(|installed| installed.removal_detail())
                        .unwrap_or_else(|| {
                            "This removes the dependency from Leyen's tracking.".to_string()
                        });

                    let confirm = gtk4::AlertDialog::builder()
                        .message(format!("Remove '{}'?", dep_id))
                        .detail(&detail)
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
                    let row_handles3 = row_handles2.clone();
                    let close_btn3 = close_btn2.clone();
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();

                    confirm.choose(Some(&dialog2), gio::Cancellable::NONE, move |result| {
                        if let Ok(1) = result {
                            dialog_busy3.set(true);
                            set_dialog_busy(
                                true,
                                &close_btn3,
                                &search_entry3,
                                &row_handles3.borrow(),
                            );
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
                            let row_handles4 = row_handles3.clone();
                            let close_btn4 = close_btn3.clone();
                            let search_entry4 = search_entry3.clone();
                            let dialog_busy4 = dialog_busy3.clone();

                            let progress_label_p = progress_label3.clone();
                            let on_progress = move |_step: usize, _total: usize, desc: String| {
                                progress_label_p.set_label(&desc);
                            };

                            let on_finish = move |success: bool, note_or_error: Option<String>| {
                                spinner4.stop();
                                spinner4.set_visible(false);
                                progress_label4.set_visible(false);
                                row4.set_sensitive(true);
                                refresh_dep_rows(&prefix4, &title4, &row_handles4.borrow());
                                dialog_busy4.set(false);
                                set_dialog_busy(
                                    false,
                                    &close_btn4,
                                    &search_entry4,
                                    &row_handles4.borrow(),
                                );
                                if success {
                                    badge4.set_visible(false);
                                    install_btn4.set_visible(true);
                                    reinstall_btn4.set_visible(false);
                                    remove_btn4.set_visible(false);
                                    let message = note_or_error
                                        .map(|note| {
                                            format!("'{}' removed successfully. {}", dep_id, note)
                                        })
                                        .unwrap_or_else(|| {
                                            format!("'{}' removed successfully.", dep_id)
                                        });
                                    overlay4.add_toast(adw::Toast::new(&message));
                                } else {
                                    install_btn4.set_visible(false);
                                    reinstall_btn4.set_visible(true);
                                    remove_btn4.set_visible(true);
                                    let msg = note_or_error
                                        .unwrap_or_else(|| "Remove failed.".to_string());
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
                        }
                    });
                });
            }

            group.add(&row);
            rows_in_group.push((row, dep_id));
        }

        page.add(&group);
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
                    let subtitle = row.subtitle().map(|s| s.to_lowercase()).unwrap_or_default();
                    title.contains(&query) || subtitle.contains(&query) || dep_id.contains(&query)
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

    refresh_dep_rows(&resolved_prefix, &title_widget, &row_handles.borrow());
    dialog.present();
}
