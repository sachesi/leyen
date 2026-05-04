use gtk4::prelude::*;
use libadwaita as adw;

use crate::models::LibraryItem;
use crate::ui::LibraryUi;
use crate::ui::components::game_card::build_game_card;
use crate::ui::components::group_card::build_group_card;
use crate::ui::utils::{
    clear_list_box, finish_list_swap, next_swap_list_box, root_library_item_cmp, running_game_map,
};

pub async fn populate_root_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    let (root_list_box, visible_page) = next_swap_list_box(
        &ui.root_list_box_primary,
        &ui.root_list_box_secondary,
        ui.root_list_showing_primary.as_ref(),
    );
    clear_list_box(root_list_box);
    ui.root_running_duration_labels.borrow_mut().clear();
    ui.root_group_running_duration_labels.borrow_mut().clear();

    let items = ui.library_state.borrow();
    if items.is_empty() {
        ui.root_content_stack.set_visible_child_name("empty");
        return;
    }

    let running_games = running_game_map().await;
    let mut sorted_items: Vec<LibraryItem> = items.clone();
    sorted_items.sort_by(|left, right| root_library_item_cmp(left, right, &running_games));

    for item in &sorted_items {
        match item {
            LibraryItem::Game(game) => {
                root_list_box.append(&build_game_card(
                    game,
                    overlay,
                    window,
                    ui,
                    &running_games,
                    &ui.root_running_duration_labels,
                ));
            }
            LibraryItem::Group(group) => {
                root_list_box.append(&build_group_card(
                    group,
                    overlay,
                    window,
                    ui,
                    &running_games,
                    &ui.root_group_running_duration_labels,
                ));
            }
        }
    }

    finish_list_swap(
        &ui.root_list_stack,
        ui.root_list_showing_primary.as_ref(),
        visible_page,
    );
    ui.root_content_stack.set_visible_child_name("list");
}
