use libadwaita as adw;
use gtk4::prelude::*;

use crate::ui::LibraryUi;
use crate::ui::components::game_card::build_game_card;
use crate::ui::utils::{
    clear_list_box, finish_list_swap, next_swap_list_box, running_game_map, find_group, game_display_cmp,
};

pub async fn populate_group_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    let (group_list_box, visible_page) = next_swap_list_box(
        &ui.group_list_box_primary,
        &ui.group_list_box_secondary,
        ui.group_list_showing_primary.as_ref(),
    );
    clear_list_box(group_list_box);
    ui.group_running_duration_labels.borrow_mut().clear();

    let Some(group_id) = ui.current_group_id.borrow().clone() else {
        return;
    };
    let items = ui.library_state.borrow();
    let Some(group) = find_group(&items, &group_id).cloned() else {
        return;
    };

    ui.title.set_title(&group.title);
    ui.title.set_subtitle("Group");

    if group.games.is_empty() {
        ui.group_content_stack.set_visible_child_name("empty");
        return;
    }

    let running_games = running_game_map().await;
    let mut games = group.games.clone();
    games.sort_by(|left, right| game_display_cmp(left, right, &running_games));

    for game in &games {
        group_list_box.append(&build_game_card(
            game,
            overlay,
            window,
            ui,
            &running_games,
            &ui.group_running_duration_labels,
        ));
    }

    finish_list_swap(
        &ui.group_list_stack,
        ui.group_list_showing_primary.as_ref(),
        visible_page,
    );
    ui.group_content_stack.set_visible_child_name("list");
}
