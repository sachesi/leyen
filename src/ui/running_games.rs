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
    content_stack: &gtk4::Stack,
    overlay: &adw::ToastOverlay,
    parent: &adw::ApplicationWindow,
    running_duration_labels: &std::rc::Rc<std::cell::RefCell<HashMap<String, gtk4::Label>>>,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    running_duration_labels.borrow_mut().clear();

    let games = load_games();
    let titles: HashMap<String, String> = games
        .into_iter()
        .map(|game| (game.id, game.title))
        .collect();

    let snapshots = running_games_snapshot();
    if snapshots.is_empty() {
        content_stack.set_visible_child_name("empty");
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
        running_duration_labels
            .borrow_mut()
            .insert(snapshot.game_id.clone(), runtime_label.clone());

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

    content_stack.set_visible_child_name("list");
}

fn update_running_durations(
    running_duration_labels: &std::rc::Rc<std::cell::RefCell<HashMap<String, gtk4::Label>>>,
) {
    let snapshots: HashMap<String, u64> = running_games_snapshot()
        .into_iter()
        .map(|snapshot| (snapshot.game_id, snapshot.elapsed_seconds))
        .collect();

    for (game_id, label) in running_duration_labels.borrow().iter() {
        if let Some(elapsed_seconds) = snapshots.get(game_id) {
            label.set_label(&format!(
                "Running for {}",
                format_duration_brief(*elapsed_seconds)
            ));
        }
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
    let empty_state = adw::StatusPage::builder()
        .icon_name("media-playback-stop-symbolic")
        .title("No running games")
        .description("Games you launch through Leyen will appear here while they are active.")
        .build();
    let content_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .hexpand(true)
        .vexpand(true)
        .build();
    content_stack.add_named(&empty_state, Some("empty"));
    content_stack.add_named(&scroll, Some("list"));
    content_stack.set_visible_child_name("empty");

    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(&content_stack));
    let running_duration_labels = std::rc::Rc::new(std::cell::RefCell::new(HashMap::new()));

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&overlay));
    window.set_content(Some(&toolbar_view));

    rebuild_running_games(
        &list_box,
        &content_stack,
        &overlay,
        parent,
        &running_duration_labels,
    );
    window.present();

    let window_ref = window.clone();
    let list_box_ref = list_box.clone();
    let content_stack_ref = content_stack.clone();
    let overlay_ref = overlay.clone();
    let parent_ref = parent.clone();
    let running_duration_labels_ref = running_duration_labels.clone();
    let mut last_version = crate::launch::running_games_version();
    glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        let current_version = crate::launch::running_games_version();
        if current_version != last_version {
            last_version = current_version;
            rebuild_running_games(
                &list_box_ref,
                &content_stack_ref,
                &overlay_ref,
                &parent_ref,
                &running_duration_labels_ref,
            );
        } else if current_version != 0 {
            update_running_durations(&running_duration_labels_ref);
        }
        glib::ControlFlow::Continue
    });
}
