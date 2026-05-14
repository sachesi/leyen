pub mod group_view;
pub mod root_view;
pub mod state;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::collections::HashSet;

pub use self::group_view::populate_group_view;
pub use self::root_view::populate_root_view;
pub use self::state::*;

use crate::launch::{launch_game, stop_game};
use crate::models::{Game, LibraryItem};
use crate::ui::utils::{
    find_group, format_duration_brief, game_is_running, group_running_started_at, running_game_map,
};

pub async fn handle_game_primary_action(game: &Game, overlay: &adw::ToastOverlay) {
    if game_is_running(&running_game_map().await, &game.id) {
        match stop_game(&game.id).await {
            Ok(true) => {
                overlay.add_toast(adw::Toast::new(&format!("Stopping {}...", game.title)));
            }
            Ok(false) => overlay.add_toast(adw::Toast::new("Game is no longer running")),
            Err(err) => {
                overlay.add_toast(adw::Toast::new(&format!("Failed to stop game: {}", err)));
            }
        }
    } else {
        launch_game(game, overlay);
    }
}

pub async fn update_running_duration_labels(ui: &LibraryUi) {
    let snapshots = running_game_map().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    if ui.current_group_id.borrow().is_some() {
        for (game_id, label) in ui.group_running_duration_labels.borrow().iter() {
            if let Some(snapshot) = snapshots.get(game_id) {
                let elapsed = now.saturating_sub(snapshot.started_at_epoch_seconds);
                label.set_label(&format!("Running for {}", format_duration_brief(elapsed)));
            }
        }
    } else {
        for (game_id, label) in ui.root_running_duration_labels.borrow().iter() {
            if let Some(snapshot) = snapshots.get(game_id) {
                let elapsed = now.saturating_sub(snapshot.started_at_epoch_seconds);
                label.set_label(&format!("Running for {}", format_duration_brief(elapsed)));
            }
        }

        let items = ui.library_state.borrow();
        for item in items.iter() {
            if let LibraryItem::Group(group) = item
                && let Some(started_at) = group_running_started_at(group, &snapshots)
                && let Some(label) = ui
                    .root_group_running_duration_labels
                    .borrow()
                    .get(&group.id)
            {
                let elapsed = now.saturating_sub(started_at);
                label.set_label(&format!("Running for {}", format_duration_brief(elapsed)));
            }
        }
    }
}

pub async fn refresh_library_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();

    let search_text = ui.search_entry.text().to_string().to_lowercase();

    glib::spawn_future_local(async move {
        let items = crate::config::load_library().await;

        let is_searching = !search_text.is_empty();

        if is_searching {
            let mut flat = Vec::new();
            let mut seen_ids = HashSet::new();
            for item in items {
                match item {
                    LibraryItem::Game(game)
                        if game.title.to_lowercase().contains(&search_text)
                            && seen_ids.insert(game.id.clone()) =>
                    {
                        flat.push(LibraryItem::Game(game));
                    }
                    LibraryItem::Group(group) => {
                        let group_matches =
                            group.title.to_lowercase().contains(&search_text);
                        for game in group.games {
                            if (group_matches
                                || game.title.to_lowercase().contains(&search_text))
                                && seen_ids.insert(game.id.clone())
                            {
                                flat.push(LibraryItem::Game(game));
                            }
                        }
                    }
                    _ => {}
                }
            }
            *ui_clone.library_state.borrow_mut() = flat;
        } else {
            *ui_clone.library_state.borrow_mut() = items;
        }

        let entering_group =
            !is_searching && ui_clone.current_group_id.borrow().is_some();

        if entering_group {
            populate_group_view(&ui_clone, &overlay_clone, &window_clone).await;
        } else {
            populate_root_view(&ui_clone, &overlay_clone, &window_clone).await;
        }

        if is_searching {
            ui_clone.stack.set_visible_child_name("root");
            ui_clone.back_btn.set_visible(false);
            ui_clone.title.set_title("Leyen");
            ui_clone.title.set_subtitle("");
        } else {
            let group_id = ui_clone.current_group_id.borrow().clone();
            if let Some(group_id) = group_id {
                if find_group(&ui_clone.library_state.borrow(), &group_id).is_none()
                {
                    *ui_clone.current_group_id.borrow_mut() = None;
                    populate_root_view(&ui_clone, &overlay_clone, &window_clone).await;
                } else {
                    ui_clone.stack.set_visible_child_name("group");
                    ui_clone.back_btn.set_visible(true);
                }
            }

            if ui_clone.current_group_id.borrow().is_none() {
                ui_clone.stack.set_visible_child_name("root");
                ui_clone.back_btn.set_visible(false);
                ui_clone.title.set_title("Leyen");
                ui_clone.title.set_subtitle("");
            }
        }

        animate_scroll_to_top(&ui_clone.toolbar_view);

        update_add_button_mode(&ui_clone);
    });
}

fn update_add_button_mode(ui: &LibraryUi) {
    let child_name = if ui.current_group_id.borrow().is_some() {
        "game"
    } else {
        "menu"
    };
    ui.add_button_stack.set_visible_child_name(child_name);
}

pub fn open_group(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    group_id: &str,
) {
    *ui.current_group_id.borrow_mut() = Some(group_id.to_string());
    let u = ui.clone();
    let o = overlay.clone();
    let w = window.clone();
    glib::spawn_future_local(async move {
        refresh_library_view(&u, &o, &w).await;
    });
}

pub fn animate_scroll_to_top(toolbar_view: &adw::ToolbarView) {
    let content = toolbar_view.content();
    let Some(sw) = content.and_then(|c| c.ancestor(gtk4::ScrolledWindow::static_type()))
    else {
        return;
    };
    let Ok(sw) = sw.downcast::<gtk4::ScrolledWindow>() else {
        return;
    };
    let vadj = sw.vadjustment();
    let current = vadj.value();
    if current <= 0.01 {
        return;
    }

    let start = std::time::Instant::now();
    let duration = std::time::Duration::from_millis(200);
    glib::timeout_add_local(std::time::Duration::from_millis(10), move || {
        let elapsed = start.elapsed().as_secs_f64();
        let t = (elapsed / duration.as_secs_f64()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
        vadj.set_value(current * (1.0 - eased));
        if t < 1.0 {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
}
