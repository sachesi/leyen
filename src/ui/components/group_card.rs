use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

use crate::icons::group_icon_file;
use crate::models::GameGroup;
use crate::ui::LibraryUi;
use crate::ui::components::icon::build_library_icon;
use crate::ui::game_dialogs::{show_delete_confirmation, show_edit_group_dialog};
use crate::ui::open_group;
use crate::ui::utils::{
    RunningGameMap, format_duration_brief, format_last_played, game_is_running, group_last_played,
    group_running_elapsed_seconds,
};

pub fn build_group_card(
    group: &GameGroup,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    ui: &LibraryUi,
    running_games: &RunningGameMap,
    running_duration_labels: &Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
) -> gtk4::Frame {
    let card = gtk4::Frame::builder()
        .hexpand(true)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    card.add_css_class("card");

    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let icon = build_library_icon(
        group_icon_file(&group.id),
        "folder",
        gtk4::Align::Start,
        group_running_elapsed_seconds(group, running_games).is_some(),
    );

    let info_column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();

    let title_label = gtk4::Label::builder()
        .label(&group.title)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();
    let running_count = group
        .games
        .iter()
        .filter(|game| game_is_running(running_games, &game.id))
        .count();
    let meta_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    let count_label = gtk4::Label::builder()
        .label(format!(
            "{} game{}",
            group.games.len(),
            if group.games.len() == 1 { "" } else { "s" }
        ))
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    meta_row.append(&count_label);
    if running_count > 0 {
        meta_row.append(
            &gtk4::Label::builder()
                .label(format!("{} running", running_count))
                .xalign(0.0)
                .css_classes(["caption", "accent"])
                .build(),
        );
    }
    let group_running_elapsed = group_running_elapsed_seconds(group, running_games);
    let status_label = gtk4::Label::builder()
        .label(if let Some(elapsed_seconds) = group_running_elapsed {
            format!("Running for {}", format_duration_brief(elapsed_seconds))
        } else {
            format_last_played(group_last_played(group))
        })
        .xalign(0.0)
        .css_classes(if group_running_elapsed.is_some() {
            ["caption", "accent"]
        } else {
            ["caption", "dim-label"]
        })
        .build();

    info_column.append(&title_label);
    info_column.append(&meta_row);
    info_column.append(&status_label);
    if group_running_elapsed.is_some() {
        running_duration_labels
            .borrow_mut()
            .insert(group.id.clone(), status_label.clone());
    }

    let open_area = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    open_area.append(&icon);
    open_area.append(&info_column);

    let button_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Center)
        .build();

    let edit_btn = gtk4::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Edit Group")
        .build();
    let delete_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete Group")
        .css_classes(["destructive-action"])
        .build();
    let open_btn = gtk4::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Open Group")
        .css_classes(["suggested-action", "circular"])
        .build();

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    open_btn
        .connect_clicked(move |_| open_group(&ui_clone, &overlay_clone, &window_clone, &group_id));

    let gesture = gtk4::GestureClick::new();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    gesture.connect_pressed(move |_, _, _, _| {
        open_group(&ui_clone, &overlay_clone, &window_clone, &group_id);
    });
    open_area.add_controller(gesture);

    let group_clone = group.clone();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    edit_btn.connect_clicked(move |_| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        let o = overlay_clone.clone();
        let g = group_clone.clone();
        glib::spawn_future_local(async move {
            show_edit_group_dialog(&w, &u, &o, &g).await;
        });
    });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    delete_btn.connect_clicked(move |_| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        let o = overlay_clone.clone();
        let gid = group_id.clone();
        glib::spawn_future_local(async move {
            show_delete_confirmation(&w, &u, &o, &gid).await;
        });
    });

    button_box.append(&edit_btn);
    button_box.append(&delete_btn);
    button_box.append(&open_btn);

    content.append(&open_area);
    content.append(&button_box);
    card.set_child(Some(&content));
    card
}
