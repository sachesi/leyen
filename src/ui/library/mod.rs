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

use crate::launch::{launch_game_headless, stop_game};
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
    crate::dbg_trace!("primary_action enter game={} accepted={}", game.id, accepted); // TEMP DEBUG
    if !accepted {
        return;
    }
    let _guard = PrimaryActionGuard(game.id.clone());
    let _t = std::time::Instant::now(); // TEMP DEBUG

    let running = game_is_running(&running_game_map().await, &game.id);
    crate::dbg_trace!("primary_action game={} running={} -> {}", game.id, running, if running { "stop" } else { "launch" }); // TEMP DEBUG
    if running {
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
        // Await the managed launch while holding the in-flight guard so rapid
        // re-clicks can't spawn duplicate concurrent launches that race
        // registration (each loser spawns then kills its own scope, flapping the
        // running-set and storming the library refresh on the GTK main thread).
        match launch_game_headless(game).await {
            Ok(report) => {
                for notice in report.notices {
                    overlay.add_toast(adw::Toast::new(&notice));
                }
            }
            Err(err) => overlay.add_toast(adw::Toast::new(&err.to_string())),
        }
    }
    crate::dbg_trace!("primary_action done game={} elapsed_ms={}", game.id, _t.elapsed().as_millis()); // TEMP DEBUG
}

/// Stops a game while holding the same in-flight guard as the library card's
/// primary action, so the Running Games window's stop button cannot issue
/// duplicate concurrent stops under rapid clicking.
pub async fn stop_game_guarded(game_id: &str, overlay: &adw::ToastOverlay) {
    let accepted =
        PRIMARY_ACTION_INFLIGHT.with(|set| set.borrow_mut().insert(game_id.to_string()));
    crate::dbg_trace!("stop_guarded enter game={} accepted={}", game_id, accepted); // TEMP DEBUG
    if !accepted {
        return;
    }
    let _guard = PrimaryActionGuard(game_id.to_string());

    match stop_game(game_id).await {
        Ok(true) => {}
        Ok(false) => overlay.add_toast(adw::Toast::new(&t!("Game is no longer running"))),
        Err(err) => {
            overlay.add_toast(adw::Toast::new(
                &t!("Failed to stop game: {}").replacen("{}", &err.to_string(), 1),
            ));
        }
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
    crate::dbg_trace!("refresh enter busy={} pending={}", ui.refresh_busy.get(), ui.refresh_pending.get()); // TEMP DEBUG
    if ui.refresh_busy.get() {
        ui.refresh_pending.set(true);
        return;
    }
    ui.refresh_busy.set(true);
    // RAII reset so a panic inside the rebuild can't leave `refresh_busy` stuck
    // true, which would permanently wedge every future library refresh.
    let _busy_guard = RefreshBusyGuard(ui.refresh_busy.clone());
    let mut passes = 0u32; // TEMP DEBUG
    loop {
        ui.refresh_pending.set(false);
        let _t = std::time::Instant::now(); // TEMP DEBUG
        run_library_refresh(ui, overlay, window).await;
        passes += 1; // TEMP DEBUG
        crate::dbg_trace!("refresh pass {} took_ms={} pending={}", passes, _t.elapsed().as_millis(), ui.refresh_pending.get()); // TEMP DEBUG
        if !ui.refresh_pending.get() {
            break;
        }
    }
    crate::dbg_trace!("refresh exit passes={}", passes); // TEMP DEBUG
}

struct RefreshBusyGuard(std::rc::Rc<std::cell::Cell<bool>>);

impl Drop for RefreshBusyGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
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
    crate::dbg_trace!("rlr start, load_library…"); // TEMP DEBUG

    {
        let items = match crate::config::load_library().await {
            Ok(items) => items,
            Err(err) => {
                overlay_clone.add_toast(adw::Toast::new(&err));
                return;
            }
        };
        crate::dbg_trace!("rlr loaded items={}", items.len()); // TEMP DEBUG

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

        crate::dbg_trace!("rlr populate entering_group={}", entering_group); // TEMP DEBUG
        if entering_group {
            populate_group_view(&ui_clone, &overlay_clone, &window_clone).await;
        } else {
            populate_root_view(&ui_clone, &overlay_clone, &window_clone).await;
        }
        crate::dbg_trace!("rlr populated"); // TEMP DEBUG

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
