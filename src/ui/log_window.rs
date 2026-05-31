use crate::t;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;

use crate::logging::{clear_log_buffer, get_log_entries, get_log_entry_count};
use crate::models::LibraryItem;

/// Scrolls the view so the end of the buffer is visible. Uses a persistent mark
/// and `scroll_to_mark`, which defers until the TextView has validated its line
/// heights — unlike poking the adjustment, which fires mid-validation with a
/// stale `upper`/`page_size` and produces jitter or under-scrolling.
fn scroll_to_end(text_view: &gtk4::TextView, buffer: &gtk4::TextBuffer, end_mark: &gtk4::TextMark) {
    buffer.move_mark(end_mark, &buffer.end_iter());
    text_view.scroll_to_mark(end_mark, 0.0, true, 0.0, 1.0);
}

fn full_rebuild(
    buffer: &gtk4::TextBuffer,
    filter: &Option<String>,
    scroll: &gtk4::ScrolledWindow,
    empty_state: &adw::StatusPage,
    text_view: &gtk4::TextView,
    end_mark: &gtk4::TextMark,
    autoscroll: &Cell<bool>,
) -> usize {
    let entries = get_log_entries();
    let lines: Vec<String> = entries
        .iter()
        .filter(|entry| filter.is_none() || filter.as_deref() == entry.game_id.as_deref())
        .map(|entry| format!("[{}] {}", entry.timestamp, entry.line))
        .collect();

    if lines.is_empty() {
        buffer.set_text("");
        scroll.set_visible(false);
        empty_state.set_visible(true);
    } else {
        buffer.set_text("");
        let mut iter = buffer.end_iter();
        for (idx, line) in lines.iter().enumerate() {
            if idx > 0 {
                buffer.insert(&mut iter, "\n");
            }
            buffer.insert(&mut iter, line);
        }
        scroll.set_visible(true);
        empty_state.set_visible(false);
        if autoscroll.get() {
            scroll_to_end(text_view, buffer, end_mark);
        }
    }
    get_log_entry_count()
}

fn try_append_new(
    buffer: &gtk4::TextBuffer,
    filter: &Option<String>,
    scroll: &gtk4::ScrolledWindow,
    empty_state: &adw::StatusPage,
    last_total: usize,
    text_view: &gtk4::TextView,
    end_mark: &gtk4::TextMark,
    autoscroll: &Cell<bool>,
) -> Option<usize> {
    let current_total = get_log_entry_count();
    if current_total == last_total {
        return Some(current_total);
    }
    if current_total < last_total {
        return None;
    }

    let entries = get_log_entries();
    let delta = current_total - last_total;
    if delta > entries.len() {
        return None;
    }

    let start_idx = entries.len() - delta;
    let mut appended = false;
    for entry in entries.iter().skip(start_idx) {
        if filter.is_none() || filter.as_deref() == entry.game_id.as_deref() {
            if !appended {
                if !scroll.is_visible() {
                    scroll.set_visible(true);
                }
                empty_state.set_visible(false);
                appended = true;
            }
            let line = format!("[{}] {}\n", entry.timestamp, entry.line);
            let mut end_iter = buffer.end_iter();
            buffer.insert(&mut end_iter, &line);
        }
    }

    if appended && autoscroll.get() {
        scroll_to_end(text_view, buffer, end_mark);
    }

    Some(current_total)
}

pub async fn show_log_window(parent: &adw::ApplicationWindow, initial_game_id: Option<&str>) {
    thread_local! {
        static ACTIVE_LOG_WINDOW: std::cell::RefCell<Option<adw::Window>> = const { std::cell::RefCell::new(None) };
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

    let library = crate::config::load_library().await.unwrap_or_default();
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
    // Right-gravity mark that rides the end of the buffer as lines are inserted.
    let end_mark = buffer.create_mark(Some("scroll-end"), &buffer.end_iter(), false);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&text_view)
        .build();

    // Follow new output only while the user is at the bottom. Scrolling up pauses
    // the follow; scrolling back to the bottom resumes it. Reading the position
    // on `value-changed` is enough — our own `scroll_to_mark` lands at the bottom,
    // so it re-arms the flag rather than fighting the user.
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

    let selected_filter = Rc::new(std::cell::RefCell::new(
        filter_ids[initial_selection as usize].clone(),
    ));

    let rendered_count = Rc::new(Cell::new(full_rebuild(
        &buffer,
        &selected_filter.borrow(),
        &scroll,
        &empty_state,
        &text_view,
        &end_mark,
        &autoscroll,
    )));

    {
        let b = buffer.clone();
        let sc = scroll.clone();
        let es = empty_state.clone();
        let sf = selected_filter.clone();
        let rc = rendered_count.clone();
        let tv = text_view.clone();
        let em = end_mark.clone();
        let asc = autoscroll.clone();
        filter_dropdown.connect_selected_notify(move |dropdown| {
            let idx = dropdown.selected() as usize;
            *sf.borrow_mut() = filter_ids.get(idx).cloned().unwrap_or(None);
            let b = b.clone();
            let sc = sc.clone();
            let es = es.clone();
            let filter = sf.borrow().clone();
            let rc = rc.clone();
            let tv = tv.clone();
            let em = em.clone();
            let asc = asc.clone();
            // Switching filter shows a fresh view — jump to its bottom.
            asc.set(true);
            glib::spawn_future_local(async move {
                rc.set(full_rebuild(&b, &filter, &sc, &es, &tv, &em, &asc));
            });
        });
    }

    {
        let b = buffer.clone();
        let sc = scroll.clone();
        let es = empty_state.clone();
        let sf = selected_filter.clone();
        let rc = rendered_count.clone();
        let tv = text_view.clone();
        let em = end_mark.clone();
        let asc = autoscroll.clone();
        clear_button.connect_clicked(move |_| {
            clear_log_buffer();
            let b = b.clone();
            let sc = sc.clone();
            let es = es.clone();
            let filter = sf.borrow().clone();
            let rc = rc.clone();
            let tv = tv.clone();
            let em = em.clone();
            let asc = asc.clone();
            asc.set(true);
            glib::spawn_future_local(async move {
                rc.set(full_rebuild(&b, &filter, &sc, &es, &tv, &em, &asc));
            });
        });
    }

    let window_ref = window.clone();
    let b = buffer.clone();
    let sc = scroll.clone();
    let es = empty_state.clone();
    let sf = selected_filter.clone();
    let rc = rendered_count.clone();
    let tv = text_view.clone();
    let em = end_mark.clone();
    let asc = autoscroll.clone();
    glib::timeout_add_seconds_local(1, move || {
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        let b = b.clone();
        let sc = sc.clone();
        let es = es.clone();
        let filter = sf.borrow().clone();
        let rc = rc.clone();
        let tv = tv.clone();
        let em = em.clone();
        let asc = asc.clone();

        glib::spawn_future_local(async move {
            let last = rc.get();
            if let Some(new_total) = try_append_new(&b, &filter, &sc, &es, last, &tv, &em, &asc) {
                rc.set(new_total);
            } else {
                rc.set(full_rebuild(&b, &filter, &sc, &es, &tv, &em, &asc));
            }
        });

        glib::ControlFlow::Continue
    });
}
