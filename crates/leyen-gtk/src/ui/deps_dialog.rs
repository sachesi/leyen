use leyen_model::t;
use libadwaita as adw;

use adw::prelude::*;
use gtk4::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::daemon::{self, DaemonEvent, gio_blocking, load_settings};
use leyen_model::deps::{
    DEP_CATEGORY_ORDER, DEP_PROFILES, DepProfile, InstalledDependency, find_installed_dependents,
    get_dep_profile, get_installed_dep, read_installed_deps, read_prefix_dep_state,
};
use leyen_model::paths::get_data_dir;

#[derive(Clone)]
struct DepRowHandle {
    dep_id: &'static str,
    action_row: adw::ActionRow,
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

#[allow(clippy::type_complexity)]
fn redistribute_rows(
    groups: &Rc<std::cell::RefCell<Vec<(adw::PreferencesGroup, Vec<(adw::ActionRow, &'static str)>)>>>,
    page: &adw::PreferencesPage,
    entries: &[&DepProfile],
    installed: &std::collections::BTreeSet<String>,
    handles: &[DepRowHandle],
    search_query: &str,
) {
    let mut new_categories: Vec<&str> = Vec::new();
    for e in entries {
        if !new_categories.contains(&e.category) {
            new_categories.push(e.category);
        }
    }
    if entries.iter().any(|e| installed.contains(e.id)) {
        new_categories.insert(0, "Installed");
    }

    // Remove all existing groups from page
    {
        let old_groups = groups.borrow();
        for (g, _) in old_groups.iter() {
            page.remove(g);
        }
    }

    // Build dep_id -> ActionRow map from handles
    let mut row_map: std::collections::HashMap<&str, adw::ActionRow> =
        std::collections::HashMap::new();
    for h in handles {
        row_map.insert(h.dep_id, h.action_row.clone());
    }

    // Build new groups
    let mut new_groups = Vec::new();
    for cat in &new_categories {
        let group = adw::PreferencesGroup::builder().title(*cat).build();
        let mut rows_in_group = Vec::new();

        for entry in entries.iter().filter(|e| {
            if *cat == "Installed" {
                installed.contains(e.id)
            } else {
                e.category == *cat && !installed.contains(e.id)
            }
        }) {
            if let Some(row) = row_map.get(entry.id) {
                row.unparent();
                group.add(row);
                rows_in_group.push((row.clone(), entry.id));
            }
        }

        page.add(&group);
        new_groups.push((group, rows_in_group));
    }

    *groups.borrow_mut() = new_groups;

    // Re-apply search filter after rebuild
    if !search_query.is_empty() {
        let groups = groups.borrow();
        for (group, rows) in groups.iter() {
            let mut any_visible = false;
            for (row, dep_id) in rows {
                let visible = {
                    let title = row.title().to_lowercase();
                    let subtitle = row.subtitle().map(|s| s.to_lowercase()).unwrap_or_default();
                    title.contains(search_query) || subtitle.contains(search_query) || dep_id.contains(search_query)
                };
                row.set_visible(visible);
                if visible {
                    any_visible = true;
                }
            }
            group.set_visible(any_visible);
        }
    }
}

fn installed_subtitle(n: usize) -> String {
    match n {
        0 => t!("No components installed"),
        1 => t!("1 component installed"),
        n => t!("{} components installed").replacen("{}", &n.to_string(), 1),
    }
}

fn escape_dep_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn sync_dep_row(
    handle: &DepRowHandle,
    installed: &std::collections::BTreeSet<String>,
    dependents: &[String],
    dep_info: Option<&InstalledDependency>,
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
            .set_tooltip_text(Some(&t!("Remove this managed dependency")));
    } else if is_installed {
        handle
            .remove_btn
            .set_tooltip_text(Some(&t!("Required by: {}").replacen("{}", &dependents.join(", "), 1)));
    } else {
        handle.remove_btn.set_tooltip_text(None);
    }
    if is_installed {
        let label = if dep_info.is_some_and(|d| d.is_prefix_integration()) {
            "✓ Integrated"
        } else {
            "✓ Installed"
        };
        handle.badge.set_label(label);
    }
}

async fn refresh_dep_rows(
    prefix_path: &str,
    title_widget: &adw::WindowTitle,
    handles: &[DepRowHandle],
) -> std::collections::BTreeSet<String> {
    let prefix_path = prefix_path.to_string();
    let state = gio_blocking(move || read_prefix_dep_state(&prefix_path)).await;
    let installed = state
        .installed
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    title_widget.set_subtitle(&installed_subtitle(installed.len()));
    for handle in handles {
        let dependents = find_installed_dependents(&state, handle.dep_id)
            .into_iter()
            .map(|dependent_id| {
                get_dep_profile(&dependent_id)
                    .map(|profile| profile.name.to_string())
                    .unwrap_or_else(|| dependent_id.to_string())
            })
            .collect::<Vec<_>>();
        let dep_info = state.installed.get(handle.dep_id);
        sync_dep_row(handle, &installed, &dependents, dep_info);
    }
    installed
}

fn set_dialog_busy(
    busy: bool,
    search_entry: &gtk4::SearchEntry,
    handles: &[DepRowHandle],
) {
    search_entry.set_sensitive(!busy);
    for handle in handles {
        handle.install_btn.set_sensitive(!busy);
        handle.reinstall_btn.set_sensitive(!busy);
        handle.remove_btn.set_sensitive(!busy);
    }
}

pub async fn open_dependencies_page(
    nav: &adw::NavigationView,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
) {
    let snapshots = crate::daemon::running_games_snapshot().await;
    if !snapshots.is_empty() {
        overlay.add_toast(adw::Toast::new(
            &t!("Dependency manager is blocked while games are running. Close all games first."),
        ));
        return;
    }

    let resolved_prefix = if !prefix_path.is_empty() {
        prefix_path.to_string()
    } else {
        let s = load_settings().await;
        if !s.default_prefix_path.is_empty() {
            s.default_prefix_path
        } else {
            get_data_dir()
                .join("prefixes")
                .join("default")
                .to_string_lossy()
                .to_string()
        }
    };

    let prefix_path_for_state = resolved_prefix.clone();
    let installed = gio_blocking(move || read_installed_deps(&prefix_path_for_state)).await;

    let subtitle = installed_subtitle(installed.len());

    let title_widget = adw::WindowTitle::new(&t!("Manage Dependencies"), &subtitle);

    let header = adw::HeaderBar::builder()
        .title_widget(&title_widget)
        .build();

    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text(t!("Search dependencies…"))
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

    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(&toolbar_view));

    // The dependency manager is a navigation subpage pushed into the opener
    // dialog's AdwNavigationView — a single dialog, never a second stacked one,
    // so the GNOME overview renders it correctly. A running operation only
    // disables the action buttons; navigating back does not interrupt it.
    let dialog_busy = Rc::new(Cell::new(false));

    let mut entries: Vec<&DepProfile> = DEP_PROFILES.iter().collect();
    entries.sort_by(|a, b| {
        dep_category_order(a.category)
            .cmp(&dep_category_order(b.category))
            .then(a.name.cmp(b.name))
    });
    let entries = Rc::new(entries);

    let mut categories: Vec<&str> = Vec::new();
    for e in entries.iter() {
        if !categories.contains(&e.category) {
            categories.push(e.category);
        }
    }
    if entries.iter().any(|e| installed.contains(e.id)) {
        categories.insert(0, "Installed");
    }

    let groups = Rc::new(std::cell::RefCell::new(
        Vec::<(adw::PreferencesGroup, Vec<(adw::ActionRow, &'static str)>)>::new(),
    ));
    let row_handles = std::rc::Rc::new(std::cell::RefCell::new(Vec::<DepRowHandle>::new()));

    for cat in &categories {
        let group = adw::PreferencesGroup::builder().title(*cat).build();
        let mut rows_in_group: Vec<(adw::ActionRow, &'static str)> = Vec::new();

        for entry in entries.iter().filter(|e| {
            if *cat == "Installed" {
                installed.contains(e.id)
            } else {
                e.category == *cat && !installed.contains(e.id)
            }
        }) {
            let dep_id = entry.id;
            let is_installed = *cat == "Installed";

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
                .label(t!("Install"))
                .css_classes(["suggested-action"])
                .valign(gtk4::Align::Center)
                .visible(!is_installed)
                .build();

            let reinstall_btn = gtk4::Button::builder()
                .icon_name("view-refresh-symbolic")
                .tooltip_text(t!("Reinstall"))
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            let remove_btn = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(t!("Remove"))
                .css_classes(["destructive-action"])
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            let cancel_btn = gtk4::Button::builder()
                .icon_name("process-stop-symbolic")
                .tooltip_text(t!("Cancel"))
                .css_classes(["destructive-action"])
                .valign(gtk4::Align::Center)
                .visible(false)
                .build();

            // Job id of the operation currently running in this row; set when the
            // daemon job starts and read by the cancel button.
            let current_job: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            {
                let current_job = current_job.clone();
                cancel_btn.connect_clicked(move |btn| {
                    if let Some(job) = current_job.borrow().clone() {
                        glib::spawn_future_local(async move {
                            crate::daemon::cancel_dep(&job).await;
                        });
                    }
                    btn.set_sensitive(false);
                });
            }
            // The cancel button is visible exactly while the operation spinner is.
            spinner
                .bind_property("visible", &cancel_btn, "visible")
                .sync_create()
                .build();

            let badge = gtk4::Label::builder()
                .label(t!("✓ Installed"))
                .css_classes(["success", "caption"])
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            row.add_suffix(&badge);
            row.add_suffix(&spinner);
            row.add_suffix(&progress_label);
            row.add_suffix(&cancel_btn);
            row.add_suffix(&install_btn);
            row.add_suffix(&reinstall_btn);
            row.add_suffix(&remove_btn);
            row_handles.borrow_mut().push(DepRowHandle {
                dep_id,
                action_row: row.clone(),
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
                let cancel_btn2 = cancel_btn.clone();
                let current_job2 = current_job.clone();
                let row2 = row.clone();
                let badge2 = badge.clone();
                let title2 = title_widget.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();
                let row_handles2 = row_handles.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();
                let groups2 = groups.clone();
                let page2 = page.clone();
                let entries2 = entries.clone();

                install_btn.connect_clicked(move |_| {
                    dialog_busy2.set(true);
                    set_dialog_busy(true, &search_entry2, &row_handles2.borrow());
                    install_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);

                    let current_job_for_op = current_job2.clone();
                    cancel_btn2.set_sensitive(true);
                    cancel_btn2.set_visible(true);

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
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();
                    let groups3 = groups2.clone();
                    let page3 = page2.clone();
                    let entries3 = entries2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, note_or_error: Option<String>| {
                        if !spinner3.is_realized() { return; }
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        let title = title3.clone();
                        let handles = row_handles3.clone();
                        let g = groups3.clone();
                        let pg = page3.clone();
                        let e = entries3.clone();
                        let search_query = search_entry3.text().to_string();
                        glib::spawn_future_local(async move {
                            let snapshot = handles.borrow().clone();
                            let inst = refresh_dep_rows(&prefix3, &title, &snapshot).await;
                            redistribute_rows(&g, &pg, &e, &inst, &handles.borrow(), &search_query);
                        });
                        dialog_busy3.set(false);
                        let busy_snapshot = row_handles3.borrow().clone();
                        set_dialog_busy(false, &search_entry3, &busy_snapshot);
                        if success {
                            badge3.set_visible(true);
                            let message = note_or_error
                                .map(|note| {
                                    t!("'{}' installed successfully. {}").replacen("{}", dep_id, 1).replacen("{}", &note, 1)
                                })
                                .unwrap_or_else(|| t!("'{}' installed successfully.").replacen("{}", dep_id, 1));
                            overlay3.add_toast(adw::Toast::new(&message));
                        } else {
                            install_btn3.set_visible(true);
                            reinstall_btn3.set_visible(false);
                            remove_btn3.set_visible(false);
                            let msg =
                                note_or_error.unwrap_or_else(|| t!("Installation failed."));
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    start_dep_job(
                        true,
                        dep_id,
                        prefix2.clone(),
                        proton2.clone(),
                        current_job_for_op,
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
                let cancel_btn2 = cancel_btn.clone();
                let current_job2 = current_job.clone();
                let row2 = row.clone();
                let badge2 = badge.clone();
                let title2 = title_widget.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();
                let row_handles2 = row_handles.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();
                let groups2 = groups.clone();
                let page2 = page.clone();
                let entries2 = entries.clone();

                reinstall_btn.connect_clicked(move |_| {
                    dialog_busy2.set(true);
                    set_dialog_busy(true, &search_entry2, &row_handles2.borrow());
                    reinstall_btn2.set_visible(false);
                    remove_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);

                    let current_job_for_op = current_job2.clone();
                    cancel_btn2.set_sensitive(true);
                    cancel_btn2.set_visible(true);

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
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();
                    let groups3 = groups2.clone();
                    let page3 = page2.clone();
                    let entries3 = entries2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, note_or_error: Option<String>| {
                        if !spinner3.is_realized() { return; }
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        let title = title3.clone();
                        let handles = row_handles3.clone();
                        let g = groups3.clone();
                        let pg = page3.clone();
                        let e = entries3.clone();
                        let search_query = search_entry3.text().to_string();
                        glib::spawn_future_local(async move {
                            let snapshot = handles.borrow().clone();
                            let inst = refresh_dep_rows(&prefix3, &title, &snapshot).await;
                            redistribute_rows(&g, &pg, &e, &inst, &handles.borrow(), &search_query);
                        });
                        dialog_busy3.set(false);
                        let busy_snapshot = row_handles3.borrow().clone();
                        set_dialog_busy(false, &search_entry3, &busy_snapshot);
                        if success {
                            badge3.set_visible(true);
                            let message = note_or_error
                                .map(|note| {
                                    t!("'{}' reinstalled successfully. {}").replacen("{}", dep_id, 1).replacen("{}", &note, 1)
                                })
                                .unwrap_or_else(|| {
                                    t!("'{}' reinstalled successfully.").replacen("{}", dep_id, 1)
                                });
                            overlay3.add_toast(adw::Toast::new(&message));
                        } else {
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let msg =
                                note_or_error.unwrap_or_else(|| t!("Reinstall failed."));
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    start_dep_job(
                        true,
                        dep_id,
                        prefix2.clone(),
                        proton2.clone(),
                        current_job_for_op,
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
                let cancel_btn2 = cancel_btn.clone();
                let dialog2 = overlay.clone();
                let row_handles2 = row_handles.clone();
                let search_entry2 = search_entry.clone();
                let dialog_busy2 = dialog_busy.clone();
                let groups2 = groups.clone();
                let page2 = page.clone();
                let entries2 = entries.clone();

                remove_btn.connect_clicked(move |_| {
                    let prefix_for_dep = prefix2.clone();
                    let dep_id_for_dep = dep_id.to_string();
                    let confirm_builder = gtk4::AlertDialog::builder()
                        .message(t!("Remove '{}'?").replacen("{}", dep_id, 1))
                        .buttons(vec![t!("Cancel"), t!("Remove")])
                        .cancel_button(0)
                        .default_button(0);

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
                    let search_entry3 = search_entry2.clone();
                    let dialog_busy3 = dialog_busy2.clone();
                    let cancel_btn3 = cancel_btn2.clone();
                    let dialog3 = dialog2.clone();
                    let groups3 = groups2.clone();
                    let page3 = page2.clone();
                    let entries3 = entries2.clone();

                    glib::spawn_future_local(async move {
                        let groups4 = groups3.clone();
                        let page4 = page3.clone();
                        let entries4 = entries3.clone();
                        let detail = gio_blocking(move || {
                            get_installed_dep(&prefix_for_dep, &dep_id_for_dep)
                                .map(|installed| installed.removal_detail())
                                .unwrap_or_else(|| {
                                    t!("This removes the dependency from Leyen's tracking.")
                                })
                        })
                        .await;

                        let confirm = confirm_builder.detail(&detail).build();
                        let root3 = dialog3
                            .root()
                            .and_then(|r| r.downcast::<gtk4::Window>().ok());
                        confirm.choose(root3.as_ref(), gio::Cancellable::NONE, move |result| {
                            if let Ok(1) = result {
                                let groups5 = groups4.clone();
                                let page5 = page4.clone();
                                let entries5 = entries4.clone();
                                dialog_busy3.set(true);
                                 set_dialog_busy(
                                     true,
                                     &search_entry3,
                                    &row_handles3.borrow(),
                                );
                                reinstall_btn3.set_visible(false);
                                remove_btn3.set_visible(false);
                                spinner3.set_visible(true);
                                cancel_btn3.set_visible(false);
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
                                let search_entry4 = search_entry3.clone();
                                let dialog_busy4 = dialog_busy3.clone();

                                let progress_label_p = progress_label3.clone();
                                let on_progress =
                                    move |_step: usize, _total: usize, desc: String| {
                                        progress_label_p.set_label(&desc);
                                    };

                                let on_finish =
                                    move |success: bool, note_or_error: Option<String>| {
                                        if !spinner4.is_realized() { return; }
                                        spinner4.stop();
                                        spinner4.set_visible(false);
                                        progress_label4.set_visible(false);
                                        row4.set_sensitive(true);
                                        let title = title4.clone();
                                        let handles = row_handles4.clone();
                                        let g = groups5.clone();
                                        let pg = page5.clone();
                                        let e = entries5.clone();
                                        let search_query = search_entry4.text().to_string();
                                        glib::spawn_future_local(async move {
                                            let snapshot = handles.borrow().clone();
                                            let inst = refresh_dep_rows(&prefix4, &title, &snapshot)
                                                .await;
                                            redistribute_rows(&g, &pg, &e, &inst, &handles.borrow(), &search_query);
                                        });
                                        dialog_busy4.set(false);
                                        let busy_snapshot = row_handles4.borrow().clone();
                                         set_dialog_busy(
                                             false,
                                             &search_entry4,
                                            &busy_snapshot,
                                        );
                                        if success {
                                            badge4.set_visible(false);
                                            install_btn4.set_visible(true);
                                            reinstall_btn4.set_visible(false);
                                            remove_btn4.set_visible(false);
                                            let message = note_or_error
                                                .map(|note| {
                                                    t!("'{}' removed successfully. {}").replacen("{}", dep_id, 1).replacen("{}", &note, 1)
                                                })
                                                .unwrap_or_else(|| {
                                                    t!("'{}' removed successfully.").replacen("{}", dep_id, 1)
                                                });
                                            overlay4.add_toast(adw::Toast::new(&message));
                                        } else {
                                            install_btn4.set_visible(false);
                                            reinstall_btn4.set_visible(true);
                                            remove_btn4.set_visible(true);
                                            let msg = note_or_error
                                                .unwrap_or_else(|| t!("Remove failed."));
                                            overlay4.add_toast(adw::Toast::new(&msg));
                                        }
                                    };

                                start_dep_job(
                                    false,
                                    dep_id,
                                    prefix3.clone(),
                                    proton3.clone(),
                                    Rc::new(RefCell::new(None)),
                                    on_progress,
                                    on_finish,
                                );
                            }
                        });
                    });
                });
            }

            group.add(&row);
            rows_in_group.push((row, dep_id));
        }

        page.add(&group);
        groups.borrow_mut().push((group, rows_in_group));
    }

    // ── Search filtering ──────────────────────────────────────────────────
    let groups_for_search = groups.clone();
    search_entry.connect_search_changed(move |entry| {
        let query = entry.text().to_lowercase();
        for (group, rows) in groups_for_search.borrow().iter() {
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

    let initial_snapshot = row_handles.borrow().clone();
    refresh_dep_rows(&resolved_prefix, &title_widget, &initial_snapshot).await;

    let nav_page = adw::NavigationPage::builder()
        .title(t!("Manage Dependencies"))
        .child(&overlay)
        .build();
    nav.push(&nav_page);
}

/// Drives a dependency install/uninstall job through the daemon: forwards
/// `DepProgress` to `on_progress` and the terminal `DepFinished` to `on_finish`,
/// recording the job id in `current_job` so the cancel button can target it.
#[allow(clippy::too_many_arguments)]
fn start_dep_job(
    install: bool,
    dep_id: &'static str,
    prefix: String,
    proton: String,
    current_job: Rc<RefCell<Option<String>>>,
    on_progress: impl Fn(usize, usize, String) + 'static,
    on_finish: impl FnOnce(bool, Option<String>) + 'static,
) {
    glib::spawn_future_local(async move {
        // Subscribe before starting so no early progress is missed.
        let events = daemon::subscribe_events();
        let job_id = if install {
            daemon::install_dep(&prefix, dep_id, &proton).await
        } else {
            daemon::uninstall_dep(&prefix, dep_id, &proton).await
        };
        if job_id.is_empty() {
            on_finish(false, Some(t!("Could not start the operation.")));
            return;
        }
        *current_job.borrow_mut() = Some(job_id.clone());

        let mut on_finish = Some(on_finish);
        while let Ok(evt) = events.recv().await {
            match evt {
                DaemonEvent::DepProgress { job_id: j, msg, .. } if j == job_id => {
                    on_progress(0, 0, msg);
                }
                DaemonEvent::DepFinished {
                    job_id: j,
                    success,
                    message,
                } if j == job_id => {
                    if current_job.borrow().as_deref() == Some(job_id.as_str()) {
                        *current_job.borrow_mut() = None;
                    }
                    if let Some(cb) = on_finish.take() {
                        let note = (!message.is_empty()).then_some(message);
                        cb(success, note);
                    }
                    break;
                }
                _ => {}
            }
        }
    });
}
