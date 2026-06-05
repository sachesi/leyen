use leyen_model::t;
use leyen_model::tn;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::glib;

use crate::daemon::{self, DaemonEvent, running_games_snapshot};
use leyen_model::icons::game_icon_path;
use leyen_model::library::flatten_games;

use super::log_window::show_log_window;
use super::utils::format_duration_brief;
use crate::ui::components::icon::build_library_icon;

/// Clears `rebuild_busy` on drop so early returns and cancellation can't leave
/// the rebuild permanently blocked.
struct RebuildGuard(Rc<Cell<bool>>);

impl Drop for RebuildGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

async fn rebuild_running_games(
    list_box: &gtk4::Box,
    content_stack: &gtk4::Stack,
    overlay: &adw::ToastOverlay,
    parent: &adw::ApplicationWindow,
    running_duration_labels: &std::rc::Rc<std::cell::RefCell<HashMap<String, gtk4::Label>>>,
    rebuild_busy: &Rc<Cell<bool>>,
) {
    // Serialize rebuilds: each clears the list then awaits before re-appending,
    // so two overlapping rebuilds would duplicate or drop cards.
    if rebuild_busy.get() {
        return;
    }
    rebuild_busy.set(true);
    let _guard = RebuildGuard(rebuild_busy.clone());

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    running_duration_labels.borrow_mut().clear();

    let mut titles = HashMap::new();
    let mut icon_paths = HashMap::new();
    let games = flatten_games(&daemon::load_library().await.unwrap_or_default());
    for game in games {
        // Candidate path only — `build_library_icon` checks existence off the main
        // thread, so the rebuild no longer stats every icon on every 1s tick.
        icon_paths.insert(game.id.clone(), game_icon_path(&game.id));
        titles.insert(game.id, game.title);
    }

    let snapshots = running_games_snapshot().await;
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
        let icon = build_library_icon(
            icon_paths.get(&snapshot.game_id).cloned(),
            "application-x-executable-symbolic",
            gtk4::Align::Center,
        );

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
            .label({
                let tracked = snapshot.tracked_pid_count as u32;
                let processes = tn!("{} process", "{} processes", tracked)
                    .replacen("{}", &tracked.to_string(), 1);
                t!("PID {} | tracking {}")
                    .replacen("{}", &snapshot.pid.to_string(), 1)
                    .replacen("{}", &processes, 1)
            })
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let elapsed = now.saturating_sub(snapshot.started_at_epoch_seconds);

        let runtime_label = gtk4::Label::builder()
            .label(t!("Running for {}").replacen("{}", &format_duration_brief(elapsed), 1))
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
            .tooltip_text(t!("View Game Logs"))
            .build();

        let stop_btn = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text(t!("Stop Game"))
            .css_classes(["destructive-action", "circular"])
            .build();

        let game_id_for_logs = snapshot.game_id.clone();
        let parent_for_logs = parent.clone();
        logs_btn.connect_clicked(move |_| {
            let parent = parent_for_logs.clone();
            let game_id = game_id_for_logs.clone();
            glib::spawn_future_local(async move {
                show_log_window(&parent, Some(&game_id)).await;
            });
        });

        let overlay_for_stop = overlay.clone();
        let leyen_id_for_stop = snapshot.leyen_id.clone();
        stop_btn.connect_clicked(move |_| {
            let leyen_id = leyen_id_for_stop.clone();
            let overlay = overlay_for_stop.clone();
            glib::spawn_future_local(async move {
                crate::ui::library::stop_game_guarded(&leyen_id, &overlay).await;
            });
        });

        actions.append(&logs_btn);
        actions.append(&stop_btn);

        content.append(&icon);
        content.append(&info);
        content.append(&actions);
        card.set_child(Some(&content));
        list_box.append(&card);
    }

    content_stack.set_visible_child_name("list");
}
pub async fn update_running_duration_labels(
    running_duration_labels: &Rc<RefCell<HashMap<String, gtk4::Label>>>,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let snapshots: HashMap<String, u64> = running_games_snapshot()
        .await
        .into_iter()
        .map(|s| (s.game_id.clone(), s.started_at_epoch_seconds))
        .collect();

    for (game_id, label) in running_duration_labels.borrow().iter() {
        if let Some(started_at) = snapshots.get(game_id) {
            let elapsed = now.saturating_sub(*started_at);
            label.set_label(&t!("Running for {}").replacen("{}", &format_duration_brief(elapsed), 1));
        }
    }
}

pub async fn show_running_games_window(parent: &adw::ApplicationWindow) {
    thread_local! {
        static ACTIVE_RUNNING_GAMES_WINDOW: std::cell::RefCell<Option<adw::Window>> = const { std::cell::RefCell::new(None) };
        static SUBSCRIPTION_ACTIVE: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
        static TIMEOUT_SOURCE_ID: std::cell::RefCell<Option<gtk4::glib::source::SourceId>> = const { std::cell::RefCell::new(None) };
    }

    if let Some(existing) = ACTIVE_RUNNING_GAMES_WINDOW.with(|w| w.borrow().clone())
        && existing.is_visible() {
            existing.present();
            return;
        }

    let window = adw::Window::builder()
        .title(t!("Leyen – Running Games"))
        .default_width(560)
        .default_height(420)
        .transient_for(parent)
        .modal(false)
        .destroy_with_parent(true)
        .build();

    ACTIVE_RUNNING_GAMES_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));

    let header = adw::HeaderBar::builder().build();

    let list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
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
        .title(t!("No running games"))
        .description(t!("Games you launch through Leyen will appear here while they are active."))
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

    let rebuild_busy = Rc::new(Cell::new(false));

    let lbox = list_box.clone();
    let cstack = content_stack.clone();
    let ov = overlay.clone();
    let p = parent.clone();
    let rdl = running_duration_labels.clone();
    let rb = rebuild_busy.clone();
    glib::spawn_future_local(async move {
        rebuild_running_games(&lbox, &cstack, &ov, &p, &rdl, &rb).await;
    });
    window.present();

    // Rebuild the list whenever running-state changes — signal-driven, no polling.
    // Guard against duplicate subscriptions if window is reopened: spawn only
    // when this call claims the flag.
    let should_spawn_subscription = SUBSCRIPTION_ACTIVE.with(|s| {
        let mut active = s.borrow_mut();
        if *active {
            false
        } else {
            *active = true;
            true
        }
    });

    if should_spawn_subscription {
        let events = daemon::subscribe_events();
        let list_box_ref = list_box.clone();
        let content_stack_ref = content_stack.clone();
        let overlay_ref = overlay.clone();
        let parent_ref = parent.clone();
        let running_duration_labels_ref = running_duration_labels.clone();
        let rebuild_busy_ref = rebuild_busy.clone();
        let window_ref = window.clone();
        glib::spawn_future_local(async move {
            while let Ok(evt) = events.recv().await {
                if !window_ref.is_visible() {
                    break;
                }
                if matches!(evt, DaemonEvent::SessionsChanged(_)) {
                    rebuild_running_games(
                        &list_box_ref,
                        &content_stack_ref,
                        &overlay_ref,
                        &parent_ref,
                        &running_duration_labels_ref,
                        &rebuild_busy_ref,
                    )
                    .await;
                }
            }
            SUBSCRIPTION_ACTIVE.with(|s| *s.borrow_mut() = false);
        });
    }

    // Cosmetic 1s tick to advance the elapsed-time labels while the window is open.
    let running_duration_labels_ref = running_duration_labels.clone();
    let window_ref = window.clone();
    let source_id = glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }
        let labels = running_duration_labels_ref.clone();
        glib::spawn_future_local(async move {
            update_running_duration_labels(&labels).await;
        });
        glib::ControlFlow::Continue
    });
    TIMEOUT_SOURCE_ID.with(|id| *id.borrow_mut() = Some(source_id));

    window.connect_close_request(move |_| {
        TIMEOUT_SOURCE_ID.with(|id| {
            if let Some(source_id) = id.borrow_mut().take() {
                source_id.remove();
            }
        });
        SUBSCRIPTION_ACTIVE.with(|s| *s.borrow_mut() = false);
        glib::Propagation::Proceed
    });
}
