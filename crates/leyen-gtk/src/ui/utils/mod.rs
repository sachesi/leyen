use leyen_ipc::RunningGameSnapshot;
use leyen_model::models::{Game, GameGroup, LibraryItem};
use leyen_model::t;
use gtk4::prelude::*;
use std::cmp::Ordering;

pub type RunningGameMap = std::collections::HashMap<String, RunningGameSnapshot>;

pub const LIST_PAGE_PRIMARY: &str = "primary";
pub const LIST_PAGE_SECONDARY: &str = "secondary";

pub async fn running_game_map() -> RunningGameMap {
    crate::daemon::running_games_snapshot()
        .await
        .into_iter()
        .map(|snapshot| (snapshot.game_id.clone(), snapshot))
        .collect()
}

pub fn clear_list_box(list_box: &gtk4::Box) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
}

pub fn next_swap_list_box<'a>(
    primary: &'a gtk4::Box,
    secondary: &'a gtk4::Box,
    showing_primary: &std::cell::Cell<bool>,
) -> (&'a gtk4::Box, &'static str) {
    if showing_primary.get() {
        (secondary, LIST_PAGE_SECONDARY)
    } else {
        (primary, LIST_PAGE_PRIMARY)
    }
}

/// Clamps `offset` to the adjustment's current scrollable range and applies it.
fn pin_scroll_offset(adjustment: &gtk4::Adjustment, offset: f64) {
    let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value(offset.clamp(adjustment.lower(), max));
}

pub fn finish_list_swap(
    list_stack: &gtk4::Stack,
    showing_primary: &std::cell::Cell<bool>,
    visible_page: &str,
) {
    // Preserve the scroll position across the double-buffer page swap. Two things
    // conspire to reset it to the top: on the first fill the scroll range is still
    // growing as cards are laid out, and the crossfade transition resets the
    // shared vadjustment mid-animation without changing the range (so no `changed`
    // fires to catch it). Capture the offset, then re-assert it immediately, on
    // every range change, and once more after the transition has finished.
    let scroll = list_stack
        .ancestor(gtk4::ScrolledWindow::static_type())
        .and_then(|widget| widget.downcast::<gtk4::ScrolledWindow>().ok());
    let saved_offset = scroll.as_ref().map(|s| s.vadjustment().value());

    list_stack.set_visible_child_name(visible_page);
    showing_primary.set(visible_page == LIST_PAGE_PRIMARY);
    // Force re-layout: cards built on hidden page may have stale allocation sizes
    if let Some(child) = list_stack.visible_child() {
        child.queue_resize();
    }

    let (Some(scroll), Some(saved_offset)) = (scroll, saved_offset) else {
        return;
    };
    let adjustment = scroll.vadjustment();
    pin_scroll_offset(&adjustment, saved_offset);

    // Re-pin while the first fill grows the range; release once it can hold the
    // offset so the handler can't fight user scrolling or accumulate.
    let handler_slot = std::rc::Rc::new(std::cell::Cell::new(None));
    let slot_for_changed = handler_slot.clone();
    let id = adjustment.connect_changed(move |adjustment| {
        pin_scroll_offset(adjustment, saved_offset);
        let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        if max + 0.5 >= saved_offset
            && let Some(id) = slot_for_changed.take()
        {
            adjustment.disconnect(id);
        }
    });
    handler_slot.set(Some(id));

    // The crossfade resets the offset partway through the animation with no
    // `changed` to catch it, so re-assert once it has settled, then release the
    // range handler. 280ms clears the 240ms crossfade.
    let adjustment_after = adjustment.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(280), move || {
        pin_scroll_offset(&adjustment_after, saved_offset);
        if let Some(id) = handler_slot.take() {
            adjustment_after.disconnect(id);
        }
    });
}

pub fn find_group<'a>(items: &'a [LibraryItem], group_id: &str) -> Option<&'a GameGroup> {
    items.iter().find_map(|item| match item {
        LibraryItem::Group(group) if group.id == group_id => Some(group),
        _ => None,
    })
}

pub fn format_playtime(playtime_seconds: u64) -> String {
    let hours = playtime_seconds / 3600;
    let minutes = (playtime_seconds % 3600) / 60;

    if hours > 0 {
        t!("Playtime: {}h {}m")
            .replacen("{}", &hours.to_string(), 1)
            .replacen("{}", &minutes.to_string(), 1)
    } else if minutes > 0 {
        t!("Playtime: {}m").replacen("{}", &minutes.to_string(), 1)
    } else {
        t!("Playtime: {}s").replacen("{}", &playtime_seconds.to_string(), 1)
    }
}

pub fn format_duration_brief(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

pub fn format_last_played(epoch_seconds: u64) -> String {
    if epoch_seconds == 0 {
        return t!("Last played: never");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(epoch_seconds);
    let delta = now.saturating_sub(epoch_seconds);

    let ago = if delta < 60 {
        t!("{}s ago").replacen("{}", &delta.to_string(), 1)
    } else if delta < 3600 {
        t!("{}m ago").replacen("{}", &(delta / 60).to_string(), 1)
    } else if delta < 86_400 {
        t!("{}h ago").replacen("{}", &(delta / 3600).to_string(), 1)
    } else {
        t!("{}d ago").replacen("{}", &(delta / 86_400).to_string(), 1)
    };

    t!("Last played: {}").replacen("{}", &ago, 1)
}

pub fn game_is_running(running_games: &RunningGameMap, game_id: &str) -> bool {
    running_games.contains_key(game_id)
}

pub fn title_cmp(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

pub fn game_display_cmp(left: &Game, right: &Game, running_games: &RunningGameMap) -> Ordering {
    game_is_running(running_games, &right.id)
        .cmp(&game_is_running(running_games, &left.id))
        .then_with(|| title_cmp(&left.title, &right.title))
}

pub fn root_library_item_cmp(
    left: &LibraryItem,
    right: &LibraryItem,
    running_games: &RunningGameMap,
) -> Ordering {
    let left_running_game = matches!(
        left,
        LibraryItem::Game(game) if game_is_running(running_games, &game.id)
    );
    let right_running_game = matches!(
        right,
        LibraryItem::Game(game) if game_is_running(running_games, &game.id)
    );

    right_running_game
        .cmp(&left_running_game)
        .then_with(|| match (left, right) {
            (LibraryItem::Group(left_group), LibraryItem::Group(right_group)) => {
                title_cmp(&left_group.title, &right_group.title)
            }
            (LibraryItem::Group(_), LibraryItem::Game(_)) => Ordering::Less,
            (LibraryItem::Game(_), LibraryItem::Group(_)) => Ordering::Greater,
            (LibraryItem::Game(left_game), LibraryItem::Game(right_game)) => {
                game_display_cmp(left_game, right_game, running_games)
            }
        })
}

pub fn running_game_elapsed_seconds(running_games: &RunningGameMap, game_id: &str) -> Option<u64> {
    running_games.get(game_id).map(|snapshot| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        now.saturating_sub(snapshot.started_at_epoch_seconds)
    })
}

pub fn group_running_elapsed_seconds(
    group: &GameGroup,
    running_games: &RunningGameMap,
) -> Option<u64> {
    group_running_started_at(group, running_games).map(|started_at| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        now.saturating_sub(started_at)
    })
}

pub fn group_running_started_at(group: &GameGroup, running_games: &RunningGameMap) -> Option<u64> {
    group
        .games
        .iter()
        .filter_map(|game| {
            running_games
                .get(&game.id)
                .map(|s| s.started_at_epoch_seconds)
        })
        .min()
}

pub fn group_last_played(group: &GameGroup) -> u64 {
    group
        .games
        .iter()
        .map(|game| game.last_played_epoch_seconds)
        .max()
        .unwrap_or(0)
}
