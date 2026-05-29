pub mod group_view;
pub mod root_view;
pub mod state;

use crate::t;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashSet;

pub use self::group_view::populate_group_view;
pub use self::root_view::populate_root_view;
pub use self::state::*;

use crate::launch::{launch_game, stop_game};
use crate::models::{Game, LibraryItem};
use crate::ui::utils::{
    find_group, format_duration_brief, game_is_running, group_running_started_at, running_game_map,
};

thread_local! {
    /// Game ids with a primary (launch/stop) action in flight. Guards against
    /// rapid double-clicks issuing duplicate stop/launch operations.
    static PRIMARY_ACTION_INFLIGHT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Removes the game id from the in-flight set on drop — covers early returns
/// and future cancellation.
struct PrimaryActionGuard(String);

impl Drop for PrimaryActionGuard {
    fn drop(&mut self) {
        PRIMARY_ACTION_INFLIGHT.with(|set| {
            set.borrow_mut().remove(&self.0);
        });
    }
}

pub async fn handle_game_primary_action(game: &Game, overlay: &adw::ToastOverlay) {
    let accepted =
        PRIMARY_ACTION_INFLIGHT.with(|set| set.borrow_mut().insert(game.id.clone()));
    if !accepted {
        return;
    }
    let _guard = PrimaryActionGuard(game.id.clone());

    if game_is_running(&running_game_map().await, &game.id) {
        match stop_game(&game.id).await {
            Ok(true) => {
                overlay.add_toast(adw::Toast::new(&t!("Stopping {}...").replacen("{}", &game.title, 1)));
            }
            Ok(false) => overlay.add_toast(adw::Toast::new(&t!("Game is no longer running"))),
            Err(err) => {
                overlay.add_toast(adw::Toast::new(&t!("Failed to stop game: {}").replacen("{}", &err.to_string(), 1)));
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
                label.set_label(&t!("Running for {}").replacen("{}", &format_duration_brief(elapsed), 1));
            }
        }
    } else {
        for (game_id, label) in ui.root_running_duration_labels.borrow().iter() {
            if let Some(snapshot) = snapshots.get(game_id) {
                let elapsed = now.saturating_sub(snapshot.started_at_epoch_seconds);
                label.set_label(&t!("Running for {}").replacen("{}", &format_duration_brief(elapsed), 1));
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
                label.set_label(&t!("Running for {}").replacen("{}", &format_duration_brief(elapsed), 1));
            }
        }
    }
}

pub async fn refresh_library_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    // Coalesce concurrent refreshes. The view rebuild clears a double-buffered
    // list box, awaits, then swaps — two overlapping rebuilds race that swap and
    // produce duplicated or missing cards. Run one at a time; collapse any
    // refreshes requested while busy into a single follow-up pass.
    if ui.refresh_busy.get() {
        ui.refresh_pending.set(true);
        return;
    }
    ui.refresh_busy.set(true);
    loop {
        ui.refresh_pending.set(false);
        run_library_refresh(ui, overlay, window).await;
        if !ui.refresh_pending.get() {
            break;
        }
    }
    ui.refresh_busy.set(false);
}

async fn run_library_refresh(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();

    let search_text = ui.search_entry.text().to_string().to_lowercase();

    {
        let items = match crate::config::load_library().await {
            Ok(items) => items,
            Err(err) => {
                overlay_clone.add_toast(adw::Toast::new(&err));
                return;
            }
        };

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
            ui_clone.title.set_title(&t!("Leyen"));
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
                ui_clone.title.set_title(&t!("Leyen"));
                ui_clone.title.set_subtitle("");
            }
        }

        update_add_button_mode(&ui_clone);
    }
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
