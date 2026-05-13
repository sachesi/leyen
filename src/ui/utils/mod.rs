use crate::launch::RunningGameSnapshot;
use crate::models::{Game, GameGroup, LibraryItem};
use gtk4::prelude::*;
use std::cmp::Ordering;

pub type RunningGameMap = std::collections::HashMap<String, RunningGameSnapshot>;

pub const LIST_PAGE_PRIMARY: &str = "primary";
pub const LIST_PAGE_SECONDARY: &str = "secondary";

pub async fn running_game_map() -> RunningGameMap {
    crate::launch::running_games_snapshot()
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

pub fn finish_list_swap(
    list_stack: &gtk4::Stack,
    showing_primary: &std::cell::Cell<bool>,
    visible_page: &str,
) {
    list_stack.set_visible_child_name(visible_page);
    showing_primary.set(visible_page == LIST_PAGE_PRIMARY);
    // Force re-layout: cards built on hidden page may have stale allocation sizes
    if let Some(child) = list_stack.visible_child() {
        child.queue_resize();
    }
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
        format!("Playtime: {}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("Playtime: {}m", minutes)
    } else {
        format!("Playtime: {}s", playtime_seconds)
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
        return "Last played: never".to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(epoch_seconds);
    let delta = now.saturating_sub(epoch_seconds);

    let ago = if delta < 60 {
        format!("{}s ago", delta)
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    };

    format!("Last played: {}", ago)
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
