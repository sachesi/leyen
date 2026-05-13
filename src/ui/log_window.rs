use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;

use crate::logging::{clear_log_buffer, get_log_entries, get_log_entry_count};
use crate::models::LibraryItem;

fn scroll_to_bottom(
    text_view: &gtk4::TextView,
    buffer: &gtk4::TextBuffer,
    scroll: &gtk4::ScrolledWindow,
) {
    let end_iter = buffer.end_iter();
    let mark = buffer.create_mark(None, &end_iter, false);
    text_view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);

    let adj = scroll.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());

    buffer.delete_mark(&mark);
}

async fn rebuild_buffer(
    buffer: &gtk4::TextBuffer,
    selected_game_id: Option<String>,
    content_stack: &gtk4::Stack,
    scroll: &gtk4::ScrolledWindow,
    text_view: &gtk4::TextView,
) -> usize {
    let entries = get_log_entries();
    let lines: Vec<String> = entries
        .iter()
        .filter(|entry| {
            selected_game_id.is_none() || (selected_game_id.as_deref() == entry.game_id.as_deref())
        })
        .map(|entry| format!("[{}] {}", entry.timestamp, entry.line))
        .collect();

    if lines.is_empty() {
        buffer.set_text("");
        content_stack.set_visible_child_name("empty");
    } else {
        buffer.set_text(&lines.join("\n"));
        content_stack.set_visible_child_name("logs");
        scroll_to_bottom(text_view, buffer, scroll);
    }
    get_log_entry_count()
}

pub async fn show_log_window(parent: &adw::ApplicationWindow, initial_game_id: Option<&str>) {
    thread_local! {
        static ACTIVE_LOG_WINDOW: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
    }

    if let Some(existing) = ACTIVE_LOG_WINDOW.with(|w| w.borrow().clone())
        && existing.is_visible() {
            existing.present();
            return;
        }

    let window = adw::Window::builder()
        .title("Leyen – Logs")
        .default_width(820)
        .default_height(440)
        .transient_for(parent)
        .modal(false)
        .build();

    ACTIVE_LOG_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));

    let library = crate::config::load_library().await;
    let mut filter_ids: Vec<Option<String>> = vec![None];
    let mut filter_labels: Vec<String> = vec!["All Logs".to_string()];

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
        .tooltip_text("Filter logs by game")
        .build();
    let clear_button = gtk4::Button::builder()
        .icon_name("edit-clear-all-symbolic")
        .tooltip_text("Clear logs")
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
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .child(&text_view)
        .build();
    let empty_state = adw::StatusPage::builder()
        .icon_name("utilities-terminal-symbolic")
        .title("No log lines to show")
        .description("New logs will appear here automatically, or choose another filter.")
        .build();
    let content_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .hexpand(true)
        .vexpand(true)
        .build();
    content_stack.add_named(&empty_state, Some("empty"));
    content_stack.add_named(&scroll, Some("logs"));
    content_stack.set_visible_child_name("empty");

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_stack));

    window.set_content(Some(&toolbar_view));
    window.present();

    let selected_filter = Rc::new(RefCell::new(filter_ids[initial_selection as usize].clone()));
    let initial_filter = selected_filter.borrow().clone();
    let rendered_count = Rc::new(Cell::new(
        rebuild_buffer(
            &buffer,
            initial_filter,
            &content_stack,
            &scroll,
            &text_view,
        )
        .await,
    ));

    let buffer_for_filter = buffer.clone();
    let content_stack_for_filter = content_stack.clone();
    let scroll_for_filter = scroll.clone();
    let text_view_for_filter = text_view.clone();
    let selected_filter_for_dropdown = selected_filter.clone();
    let filter_ids_for_dropdown = filter_ids.clone();
    let rendered_count_for_dropdown = rendered_count.clone();
    filter_dropdown.connect_selected_notify(move |dropdown| {
        let selected = dropdown.selected() as usize;
        let game_id = filter_ids_for_dropdown
            .get(selected)
            .cloned()
            .unwrap_or(None);
        *selected_filter_for_dropdown.borrow_mut() = game_id;

        let b = buffer_for_filter.clone();
        let s = selected_filter_for_dropdown.borrow().clone();
        let cs = content_stack_for_filter.clone();
        let sc = scroll_for_filter.clone();
        let tv = text_view_for_filter.clone();
        let rc = rendered_count_for_dropdown.clone();
        glib::spawn_future_local(async move {
            rc.set(rebuild_buffer(&b, s, &cs, &sc, &tv).await);
        });
    });

    let buffer_for_clear = buffer.clone();
    let content_stack_for_clear = content_stack.clone();
    let scroll_for_clear = scroll.clone();
    let text_view_for_clear = text_view.clone();
    let selected_filter_for_clear = selected_filter.clone();
    let rendered_count_for_clear = rendered_count.clone();
    clear_button.connect_clicked(move |_| {
        let b = buffer_for_clear.clone();
        let s = selected_filter_for_clear.borrow().clone();
        let cs = content_stack_for_clear.clone();
        let sc = scroll_for_clear.clone();
        let tv = text_view_for_clear.clone();
        let rc = rendered_count_for_clear.clone();
        glib::spawn_future_local(async move {
            clear_log_buffer();
            rc.set(rebuild_buffer(&b, s, &cs, &sc, &tv).await);
        });
    });

    let window_ref = window.clone();
    let buffer_for_tick = buffer.clone();
    let content_stack_for_tick = content_stack.clone();
    let scroll_for_tick = scroll.clone();
    let text_view_for_tick = text_view.clone();
    glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        let b = buffer_for_tick.clone();
        let s = selected_filter.borrow().clone();
        let cs = content_stack_for_tick.clone();
        let sc = scroll_for_tick.clone();
        let tv = text_view_for_tick.clone();
        let rc = rendered_count.clone();

        glib::spawn_future_local(async move {
            let from = rc.get();
            let current = get_log_entry_count();
            if current != from {
                // If it's a small update, we could append, but for now just rebuild
                // but at least it's from memory!
                rc.set(rebuild_buffer(&b, s, &cs, &sc, &tv).await);
            }
        });

        glib::ControlFlow::Continue
    });
}
