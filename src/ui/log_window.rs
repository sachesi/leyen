use libadwaita as adw;

use adw::prelude::*;
use gtk4::glib;

use crate::logging::{clear_log_buffer, get_log_buffer};

/// Show (or re-present) the always-on-top, read-only log viewer window.
///
/// The window polls the global log buffer every second and appends new lines.
/// The text view is non-editable so the user cannot modify the output.
pub fn show_log_window(parent: &adw::ApplicationWindow) {
    let window = adw::Window::builder()
        .title("Leyen – Logs")
        .default_width(700)
        .default_height(400)
        .transient_for(parent)
        .modal(false)
        .build();

    let header = adw::HeaderBar::builder().build();
    let clear_button = gtk4::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("Clear logs")
        .build();
    header.pack_start(&clear_button);

    let text_view = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(4)
        .bottom_margin(4)
        .left_margin(8)
        .right_margin(8)
        .build();

    let buffer = text_view.buffer();

    // Populate with existing log content
    let existing = get_log_buffer();
    if !existing.is_empty() {
        buffer.set_text(&existing.join("\n"));
    }
    // Track how many lines we have already rendered
    let rendered_count = std::cell::Cell::new(existing.len());
    let rendered_count_for_clear = rendered_count.clone();
    let buffer_for_clear = buffer.clone();
    clear_button.connect_clicked(move |_| {
        clear_log_buffer();
        buffer_for_clear.set_text("");
        rendered_count_for_clear.set(0);
    });

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

    // Poll the global buffer once per second for new lines.
    let window_ref = window.clone();
    let scroll_clone = scroll.clone();
    glib::timeout_add_seconds_local(1, move || {
        // If the window has been closed / destroyed, stop the timer.
        if !window_ref.is_visible() {
            return glib::ControlFlow::Break;
        }

        let all_lines = get_log_buffer();
        let already = rendered_count.get();

        if all_lines.len() > already {
            let new_text = all_lines[already..].join("\n");
            let mut end_iter = buffer.end_iter();
            if already > 0 {
                buffer.insert(&mut end_iter, "\n");
                end_iter = buffer.end_iter();
            }
            buffer.insert(&mut end_iter, &new_text);
            rendered_count.set(all_lines.len());

            // Auto-scroll to bottom
            let adj = scroll_clone.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        }

        glib::ControlFlow::Continue
    });
}
