use leyen_model::t;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;

use crate::daemon::{self, DaemonEvent};
use leyen_ipc::LogEntry;
use leyen_model::models::LibraryItem;

/// Scrolls the view so the end of the buffer is visible.
fn scroll_to_end(text_view: &gtk4::TextView, buffer: &gtk4::TextBuffer, end_mark: &gtk4::TextMark) {
    buffer.move_mark(end_mark, &buffer.end_iter());
    text_view.scroll_to_mark(end_mark, 0.0, true, 0.0, 1.0);
}

fn entry_matches(entry: &LogEntry, filter: &Option<String>) -> bool {
    match filter {
        None => true,
        Some(id) => entry.game_id == *id,
    }
}

/// Appends matching entries to the buffer, toggling the empty state and
/// autoscrolling if pinned to the bottom.
#[allow(clippy::too_many_arguments)]
fn append_entries(
    buffer: &gtk4::TextBuffer,
    filter: &Option<String>,
    scroll: &gtk4::ScrolledWindow,
    empty_state: &adw::StatusPage,
    entries: &[LogEntry],
    text_view: &gtk4::TextView,
    end_mark: &gtk4::TextMark,
    autoscroll: &Cell<bool>,
) {
    let mut appended = false;
    for entry in entries.iter().filter(|e| entry_matches(e, filter)) {
        if !appended {
            scroll.set_visible(true);
            empty_state.set_visible(false);
            appended = true;
        }
        let line = format!("[{}] {}\n", entry.timestamp, entry.line);
        let mut end_iter = buffer.end_iter();
        buffer.insert(&mut end_iter, &line);
    }
    if appended && autoscroll.get() {
        scroll_to_end(text_view, buffer, end_mark);
    } else if buffer.char_count() == 0 {
        scroll.set_visible(false);
        empty_state.set_visible(true);
    }
}

pub async fn show_log_window(parent: &adw::ApplicationWindow, initial_game_id: Option<&str>) {
    thread_local! {
        static ACTIVE_LOG_WINDOW: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
    }

    if let Some(existing) = ACTIVE_LOG_WINDOW.with(|w| w.borrow().clone())
        && existing.is_visible()
    {
        existing.present();
        return;
    }

    let window = adw::Window::builder()
        .title(t!("Leyen – Logs"))
        .default_width(820)
        .default_height(440)
        .transient_for(parent)
        .modal(false)
        .destroy_with_parent(true)
        .build();

    ACTIVE_LOG_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));

    let library = daemon::load_library().await.unwrap_or_default();
    let mut filter_ids: Vec<Option<String>> = vec![None];
    let mut filter_labels: Vec<String> = vec![t!("All Logs")];

    for item in &library {
        match item {
            LibraryItem::Game(game) => {
                filter_ids.push(Some(game.id.clone()));
                filter_labels.push(game.title.clone());
            }
            LibraryItem::Group(group) => {
                for game in &group.games {
                    filter_ids.push(Some(game.id.clone()));
                    filter_labels.push(format!("{}: {}", group.title, game.title));
                }
            }
        }
    }

    let filter_refs: Vec<&str> = filter_labels.iter().map(|label| label.as_str()).collect();
    let filter_model = gtk4::StringList::new(&filter_refs);

    let initial_selection = initial_game_id
        .and_then(|game_id| {
            filter_ids
                .iter()
                .position(|candidate| candidate.as_deref() == Some(game_id))
        })
        .unwrap_or(0) as u32;

    let header = adw::HeaderBar::builder().build();
    let filter_dropdown = gtk4::DropDown::builder()
        .model(&filter_model)
        .selected(initial_selection)
        .tooltip_text(t!("Filter logs by game"))
        .build();
    let clear_button = gtk4::Button::builder()
        .icon_name("edit-clear-all-symbolic")
        .tooltip_text(t!("Clear logs"))
        .build();
    header.pack_start(&filter_dropdown);
    header.pack_end(&clear_button);

    let text_view = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(4)
        .bottom_margin(4)
        .left_margin(20)
        .right_margin(12)
        .build();

    let buffer = text_view.buffer();
    let end_mark = buffer.create_mark(Some("scroll-end"), &buffer.end_iter(), false);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&text_view)
        .build();

    let autoscroll = Rc::new(Cell::new(true));
    {
        let autoscroll = autoscroll.clone();
        scroll.vadjustment().connect_value_changed(move |adj| {
            let at_bottom = adj.upper() <= adj.page_size()
                || adj.value() + adj.page_size() >= adj.upper() - 8.0;
            autoscroll.set(at_bottom);
        });
    }

    let empty_state = adw::StatusPage::builder()
        .icon_name("utilities-terminal-symbolic")
        .title(t!("No log lines to show"))
        .description(t!("New logs will appear here automatically, or choose another filter."))
        .hexpand(true)
        .vexpand(true)
        .build();

    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .build();
    content_box.append(&scroll);
    content_box.append(&empty_state);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));

    window.set_content(Some(&toolbar_view));
    window.present();

    let selected_filter = Rc::new(RefCell::new(filter_ids[initial_selection as usize].clone()));
    // Monotonic pull offset shared across the initial render, filter changes and
    // the LogsAppended-driven incremental appends.
    let offset = Rc::new(Cell::new(0u64));

    // Rebuilds the whole view for the current filter from the daemon's retained
    // log buffer.
    let rebuild = {
        let buffer = buffer.clone();
        let scroll = scroll.clone();
        let empty_state = empty_state.clone();
        let text_view = text_view.clone();
        let end_mark = end_mark.clone();
        let autoscroll = autoscroll.clone();
        let selected_filter = selected_filter.clone();
        let offset = offset.clone();
        move || {
            let buffer = buffer.clone();
            let scroll = scroll.clone();
            let empty_state = empty_state.clone();
            let text_view = text_view.clone();
            let end_mark = end_mark.clone();
            let autoscroll = autoscroll.clone();
            let selected_filter = selected_filter.clone();
            let offset = offset.clone();
            glib::spawn_future_local(async move {
                let (next, entries) = daemon::get_logs(0).await;
                buffer.set_text("");
                let filter = selected_filter.borrow().clone();
                append_entries(
                    &buffer, &filter, &scroll, &empty_state, &entries, &text_view, &end_mark,
                    &autoscroll,
                );
                offset.set(next);
            });
        }
    };

    rebuild();

    {
        let rebuild = rebuild.clone();
        let selected_filter = selected_filter.clone();
        let autoscroll = autoscroll.clone();
        filter_dropdown.connect_selected_notify(move |dropdown| {
            let idx = dropdown.selected() as usize;
            *selected_filter.borrow_mut() = filter_ids.get(idx).cloned().unwrap_or(None);
            autoscroll.set(true);
            rebuild();
        });
    }

    {
        let rebuild = rebuild.clone();
        let autoscroll = autoscroll.clone();
        clear_button.connect_clicked(move |_| {
            let rebuild = rebuild.clone();
            let autoscroll = autoscroll.clone();
            glib::spawn_future_local(async move {
                daemon::clear_logs().await;
                autoscroll.set(true);
                rebuild();
            });
        });
    }

    // Incremental appends driven by LogsAppended (notify-then-pull).
    {
        let events = daemon::subscribe_events();
        let buffer = buffer.clone();
        let scroll = scroll.clone();
        let empty_state = empty_state.clone();
        let text_view = text_view.clone();
        let end_mark = end_mark.clone();
        let autoscroll = autoscroll.clone();
        let selected_filter = selected_filter.clone();
        let offset = offset.clone();
        let window_ref = window.clone();
        glib::spawn_future_local(async move {
            while let Ok(evt) = events.recv().await {
                if !window_ref.is_visible() {
                    break;
                }
                if matches!(evt, DaemonEvent::LogsAppended(_)) {
                    let (next, entries) = daemon::get_logs(offset.get()).await;
                    let filter = selected_filter.borrow().clone();
                    append_entries(
                        &buffer, &filter, &scroll, &empty_state, &entries, &text_view, &end_mark,
                        &autoscroll,
                    );
                    offset.set(next);
                }
            }
        });
    }
}
