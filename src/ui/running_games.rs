use std::collections::HashMap;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::glib;

use crate::config::load_games;
use crate::launch::{running_games_snapshot, stop_game};

use super::log_window::show_log_window;

fn format_duration_brief(total_seconds: u64) -> String {
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

fn rebuild_running_games(
    list_box: &gtk4::Box,
    overlay: &adw::ToastOverlay,
    parent: &adw::ApplicationWindow,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let games = load_games();
    let titles: HashMap<String, String> = games
        .into_iter()
        .map(|game| (game.id, game.title))
        .collect();

    let snapshots = running_games_snapshot();
    if snapshots.is_empty() {
        let empty = gtk4::Label::builder()
            .label("No running games")
            .css_classes(["title-4", "dim-label"])
            .halign(gtk4::Align::Center)
            .margin_top(24)
            .margin_bottom(24)
            .build();
        list_box.append(&empty);
        return;
    }

    for snapshot in snapshots {
        let title = titles
            .get(&snapshot.game_id)
            .cloned()
            .unwrap_or_else(|| snapshot.game_id.clone());

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

        let info = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();

        let title_label = gtk4::Label::builder()
            .label(&title)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();

        let pid_label = gtk4::Label::builder()
            .label(format!(
                "PID {} | tracking {} process{}",
                snapshot.pid,
                snapshot.tracked_pid_count,
                if snapshot.tracked_pid_count == 1 {
                    ""
                } else {
                    "es"
                }
            ))
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();

        let runtime_label = gtk4::Label::builder()
            .label(format!(
                "Running for {}",
                format_duration_brief(snapshot.elapsed_seconds)
            ))
            .xalign(0.0)
            .css_classes(["caption", "accent"])
            .build();

        info.append(&title_label);
        info.append(&pid_label);
        info.append(&runtime_label);

        let actions = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let logs_btn = gtk4::Button::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text("View Game Logs")
            .build();

        let stop_btn = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Stop Game")
            .css_classes(["destructive-action", "circular"])
            .build();

        let game_id_for_logs = snapshot.game_id.clone();
        let parent_for_logs = parent.clone();
        logs_btn.connect_clicked(move |_| {
            show_log_window(&parent_for_logs, Some(&game_id_for_logs));
        });

        let overlay_for_stop = overlay.clone();
        let game_id_for_stop = snapshot.game_id.clone();
        let title_for_stop = title.clone();
        stop_btn.connect_clicked(move |_| match stop_game(&game_id_for_stop) {
            Ok(true) => overlay_for_stop
                .add_toast(adw::Toast::new(&format!("Stopping {}...", title_for_stop))),
            Ok(false) => overlay_for_stop.add_toast(adw::Toast::new("Game is no longer running")),
            Err(err) => overlay_for_stop
                .add_toast(adw::Toast::new(&format!("Failed to stop game: {}", err))),
        });

        actions.append(&logs_btn);
        actions.append(&stop_btn);

        content.append(&info);
        content.append(&actions);
        card.set_child(Some(&content));
        list_box.append(&card);
    }
}

pub fn show_running_games_window(parent: &adw::ApplicationWindow) {
    let window = adw::Window::builder()
        .title("Leyen – Running Games")
        .default_width(560)
        .default_height(420)
        .transient_for(parent)
        .modal(false)
        .build();

    let header = adw::HeaderBar::builder().build();

    let list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&list_box)
        .build();

    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(&scroll));

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&overlay));
    window.set_content(Some(&toolbar_view));

    rebuild_running_games(&list_box, &overlay, parent);
    window.present();

    let window_ref = window.clone();
    let list_box_ref = list_box.clone();
    let overlay_ref = overlay.clone();
    let parent_ref = parent.clone();
    glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        rebuild_running_games(&list_box_ref, &overlay_ref, &parent_ref);
        glib::ControlFlow::Continue
    });
}
