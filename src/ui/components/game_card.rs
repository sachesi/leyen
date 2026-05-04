use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

use crate::icons::game_icon_file;
use crate::models::Game;
use crate::ui::LibraryUi;
use crate::ui::components::icon::build_library_icon;
use crate::ui::game_dialogs::{show_delete_confirmation, show_edit_game_dialog};
use crate::ui::handle_game_primary_action;
use crate::ui::utils::{
    RunningGameMap, format_duration_brief, format_last_played, format_playtime, game_is_running,
    running_game_elapsed_seconds,
};

pub fn build_game_card(
    game: &Game,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    ui: &LibraryUi,
    running_games: &RunningGameMap,
    running_duration_labels: &Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
) -> gtk4::Frame {
    let game_running = game_is_running(running_games, &game.id);
    let card = gtk4::Frame::builder()
        .hexpand(true)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    card.add_css_class("card");
    if game_running {
        card.add_css_class("running-card");
    }

    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let icon = build_library_icon(
        game_icon_file(&game.id),
        "application-x-executable-symbolic",
        gtk4::Align::Start,
        game_running,
    );

    let info_column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    let open_area = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();

    let title_label = gtk4::Label::builder()
        .label(&game.title)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();

    info_column.append(&title_label);
    info_column.append(
        &gtk4::Label::builder()
            .label(format_playtime(game.playtime_seconds))
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    info_column.append(&{
        let status_label = gtk4::Label::builder()
            .label(if game_running {
                format!(
                    "Running for {}",
                    format_duration_brief(
                        running_game_elapsed_seconds(running_games, &game.id).unwrap_or(0)
                    )
                )
            } else {
                format_last_played(game.last_played_epoch_seconds)
            })
            .xalign(0.0)
            .css_classes(if game_running {
                ["caption", "accent"]
            } else {
                ["caption", "dim-label"]
            })
            .build();
        if game_running {
            running_duration_labels
                .borrow_mut()
                .insert(game.id.clone(), status_label.clone());
        }
        status_label
    });

    open_area.append(&icon);
    open_area.append(&info_column);

    let gesture = gtk4::GestureClick::new();
    let game_clone = game.clone();
    let overlay_clone = overlay.clone();
    gesture.connect_pressed(move |_, _, _, _| {
        let game = game_clone.clone();
        let overlay = overlay_clone.clone();
        glib::spawn_future_local(async move {
            handle_game_primary_action(&game, &overlay).await;
        });
    });
    open_area.add_controller(gesture);

    let button_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Center)
        .build();

    let edit_btn = gtk4::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Edit Game")
        .build();
    let delete_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete Game")
        .css_classes(["destructive-action"])
        .build();
    let play_btn = gtk4::Button::builder()
        .icon_name(if game_running {
            "media-playback-stop-symbolic"
        } else {
            "media-playback-start-symbolic"
        })
        .css_classes(if game_running {
            ["destructive-action", "circular"]
        } else {
            ["suggested-action", "circular"]
        })
        .tooltip_text(if game_running {
            "Stop Game"
        } else {
            "Launch Game"
        })
        .build();

    let game_clone = game.clone();
    let overlay_clone = overlay.clone();
    play_btn.connect_clicked(move |_| {
        let game = game_clone.clone();
        let overlay = overlay_clone.clone();
        glib::spawn_future_local(async move {
            handle_game_primary_action(&game, &overlay).await;
        });
    });

    let game_clone = game.clone();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    edit_btn.connect_clicked(move |_| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        let o = overlay_clone.clone();
        let g = game_clone.clone();
        glib::spawn_future_local(async move {
            show_edit_game_dialog(&w, &u, &o, &g).await;
        });
    });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let game_id = game.id.clone();
    delete_btn.connect_clicked(move |_| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        let o = overlay_clone.clone();
        let gid = game_id.clone();
        glib::spawn_future_local(async move {
            show_delete_confirmation(&w, &u, &o, &gid).await;
        });
    });

    button_box.append(&edit_btn);
    button_box.append(&delete_btn);
    button_box.append(&play_btn);

    content.append(&open_area);
    content.append(&button_box);
    card.set_child(Some(&content));
    card
}
