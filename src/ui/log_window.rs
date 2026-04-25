use std::cell::{Cell, RefCell};

use libadwaita as adw;

use adw::prelude::*;
use gtk4::glib;

use crate::config::load_games;
use crate::logging::{clear_log_buffer, get_log_entries};

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

fn rebuild_buffer(
    buffer: &gtk4::TextBuffer,
    selected_game_id: Option<&str>,
    scroll: &gtk4::ScrolledWindow,
    text_view: &gtk4::TextView,
) -> usize {
    let entries = get_log_entries();
    let lines: Vec<String> = entries
        .iter()
        .filter(|entry| {
            selected_game_id.is_none_or(|game_id| entry.game_id.as_deref() == Some(game_id))
        })
        .map(|entry| entry.line.clone())
        .collect();

    buffer.set_text(&lines.join("\n"));
    scroll_to_bottom(text_view, buffer, scroll);
    entries.len()
}

pub fn show_log_window(parent: &adw::ApplicationWindow, initial_game_id: Option<&str>) {
    let window = adw::Window::builder()
        .title("Leyen – Logs")
        .default_width(820)
        .default_height(440)
        .transient_for(parent)
        .modal(false)
        .build();

    let games = load_games();
    let mut filter_ids: Vec<Option<String>> = vec![None];
    let mut filter_labels: Vec<String> = vec!["All Logs".to_string()];
    for game in &games {
        filter_ids.push(Some(game.id.clone()));
        filter_labels.push(game.title.clone());
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

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&scroll));

    window.set_content(Some(&toolbar_view));
    window.present();

    let selected_filter = RefCell::new(filter_ids[initial_selection as usize].clone());
    let rendered_count = Cell::new(rebuild_buffer(
        &buffer,
        selected_filter.borrow().as_deref(),
        &scroll,
        &text_view,
    ));

    let buffer_for_filter = buffer.clone();
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
        rendered_count_for_dropdown.set(rebuild_buffer(
            &buffer_for_filter,
            selected_filter_for_dropdown.borrow().as_deref(),
            &scroll_for_filter,
            &text_view_for_filter,
        ));
    });

    let buffer_for_clear = buffer.clone();
    let scroll_for_clear = scroll.clone();
    let text_view_for_clear = text_view.clone();
    let selected_filter_for_clear = selected_filter.clone();
    let rendered_count_for_clear = rendered_count.clone();
    clear_button.connect_clicked(move |_| {
        clear_log_buffer();
        rendered_count_for_clear.set(rebuild_buffer(
            &buffer_for_clear,
            selected_filter_for_clear.borrow().as_deref(),
            &scroll_for_clear,
            &text_view_for_clear,
        ));
    });

    let window_ref = window.clone();
    let buffer_for_tick = buffer.clone();
    let scroll_for_tick = scroll.clone();
    let text_view_for_tick = text_view.clone();
    glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        let entry_count = get_log_entries().len();
        if entry_count != rendered_count.get() {
            rendered_count.set(rebuild_buffer(
                &buffer_for_tick,
                selected_filter.borrow().as_deref(),
                &scroll_for_tick,
                &text_view_for_tick,
            ));
        }

        glib::ControlFlow::Continue
    });
}
