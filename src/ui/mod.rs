pub mod deps_dialog;
pub mod game_dialogs;
pub mod settings;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::glib;

use crate::config::load_games;
use crate::launch::launch_game;
use crate::models::Game;
use crate::umu::{is_umu_run_available, UMU_DOWNLOADING};

use self::game_dialogs::{show_add_game_dialog, show_delete_confirmation, show_edit_game_dialog};
use self::settings::show_global_settings;

pub fn build_ui(app: &adw::Application) {
    // Hide the built-in pencil/edit indicator that AdwEntryRow shows by default
    let css = gtk4::CssProvider::new();
    css.load_from_string(
        "image.edit-icon { min-width: 0px; min-height: 0px; \
         margin: 0px; padding: 0px; opacity: 0; }",
    );
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let header = adw::HeaderBar::builder().build();

    let add_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Game")
        .build();

    let settings_btn = gtk4::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Preferences")
        .build();

    header.pack_start(&add_btn);
    header.pack_end(&settings_btn);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);

    let clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();

    let game_list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();

    let empty_state = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .spacing(6)
        .build();

    let empty_label = gtk4::Label::builder()
        .label("No games added yet")
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .css_classes(["title-3"])
        .build();

    let empty_hint = gtk4::Label::builder()
        .label("Add a game to see it listed here.")
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .css_classes(["dim-label"])
        .build();

    empty_state.append(&empty_label);
    empty_state.append(&empty_hint);

    clamp.set_child(Some(&game_list_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));

    // Banner shown while umu-launcher is being downloaded in the background.
    let download_banner = adw::Banner::builder()
        .title("Downloading umu-launcher… Please wait before starting games.")
        .revealed(UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed))
        .build();
    toolbar_view.add_top_bar(&download_banner);

    toolbar_view.set_content(Some(&toast_overlay));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Leyen")
        .default_width(700)
        .default_height(600)
        .content(&toolbar_view)
        .build();

    // Load games from disk and populate the list
    let games = load_games();
    populate_game_list(
        &game_list_box,
        &empty_state,
        &games,
        &toast_overlay,
        &window,
    );

    // Poll every 2 seconds; hide the banner and show a toast once the download
    // completes (or if it was never needed).
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        let banner_clone = download_banner.clone();
        let overlay_clone = toast_overlay.clone();
        glib::timeout_add_seconds_local(2, move || {
            if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
                return glib::ControlFlow::Continue;
            }
            banner_clone.set_revealed(false);
            if is_umu_run_available() {
                overlay_clone.add_toast(adw::Toast::new("umu-launcher downloaded. Ready to play!"));
            } else {
                overlay_clone.add_toast(adw::Toast::new(
                    "Failed to download umu-launcher. Check your internet connection.",
                ));
            }
            glib::ControlFlow::Break
        });
    }

    /* --- EVENT HANDLERS --- */

    let window_clone = window.clone();
    let overlay_for_settings = toast_overlay.clone();
    settings_btn.connect_clicked(move |_| {
        show_global_settings(&window_clone, &overlay_for_settings);
    });

    let window_clone_2 = window.clone();
    let list_box_clone = game_list_box.clone();
    let empty_state_clone = empty_state.clone();
    let overlay_clone = toast_overlay.clone();
    add_btn.connect_clicked(move |_| {
        show_add_game_dialog(
            &window_clone_2,
            &list_box_clone,
            &empty_state_clone,
            &overlay_clone,
        );
    });

    window.present();
}

// --- DYNAMIC UI GENERATOR ---

pub fn populate_game_list(
    list_box: &gtk4::Box,
    empty_state: &gtk4::Box,
    games: &[Game],
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    // Clear existing children
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    if games.is_empty() {
        list_box.append(empty_state);
        return;
    }

    for game in games {
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

        let icon = gtk4::Image::builder()
            .icon_name("application-x-executable-symbolic")
            .pixel_size(48)
            .valign(gtk4::Align::Start)
            .build();

        let info_column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();

        let title_label = gtk4::Label::builder()
            .label(&game.title)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();

        let path_label = gtk4::Label::builder()
            .label(&game.exe_path)
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();

        info_column.append(&title_label);
        info_column.append(&path_label);

        // Button box for actions
        let button_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let edit_btn = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .valign(gtk4::Align::Center)
            .tooltip_text("Edit Game")
            .build();

        let delete_btn = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk4::Align::Center)
            .tooltip_text("Delete Game")
            .css_classes(["destructive-action"])
            .build();

        let play_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .css_classes(["suggested-action", "circular"])
            .valign(gtk4::Align::Center)
            .tooltip_text("Launch Game")
            .build();

        // Launch Logic!
        let game_clone = game.clone();
        let overlay_clone = overlay.clone();
        play_btn.connect_clicked(move |_| {
            launch_game(&game_clone, &overlay_clone);
        });

        // Edit Logic
        let game_clone = game.clone();
        let list_box_clone = list_box.clone();
        let empty_state_clone = empty_state.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        edit_btn.connect_clicked(move |_| {
            show_edit_game_dialog(
                &window_clone,
                &list_box_clone,
                &empty_state_clone,
                &overlay_clone,
                &game_clone,
            );
        });

        // Delete Logic
        let game_id = game.id.clone();
        let list_box_clone = list_box.clone();
        let empty_state_clone = empty_state.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        delete_btn.connect_clicked(move |_| {
            show_delete_confirmation(
                &window_clone,
                &list_box_clone,
                &empty_state_clone,
                &overlay_clone,
                &game_id,
            );
        });

        button_box.append(&edit_btn);
        button_box.append(&delete_btn);
        button_box.append(&play_btn);

        content.append(&icon);
        content.append(&info_column);
        content.append(&button_box);

        card.set_child(Some(&content));
        list_box.append(&card);
    }
}
