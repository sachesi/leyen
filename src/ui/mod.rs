pub mod components;
pub mod deps_dialog;
pub mod game_dialogs;
pub mod library;
pub mod log_window;
pub mod running_games;
pub mod settings;
pub mod utils;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use self::library::*;
pub use self::utils::*;

use self::game_dialogs::{AddLibraryItemKind, show_add_library_item_dialog};
use self::log_window::show_log_window;
use self::running_games::show_running_games_window;
use self::settings::show_global_settings;

use std::sync::atomic::Ordering;

use crate::runtime::umu::{UMU_DOWNLOADING, WINETRICKS_DOWNLOADING};

pub fn build_ui(app: &adw::Application) {
    let css = gtk4::CssProvider::new();
    css.load_from_string(&format!(
        "image.edit-icon {{ min-width: 0px; min-height: 0px; margin: 0px; padding: 0px; opacity: 0; }} \
         .library-icon-frame {{ min-width: {0}px; min-height: {0}px; border-radius: {1}px; padding: 0px; margin: 0px; background: transparent; }} \
         .library-icon-media {{ border-radius: {1}px; }} \
         .card {{ border-radius: 12px; transition: all 200ms ease; }} \
         .card:hover {{ background-color: alpha(@window_fg_color, 0.05); }} \
         .running-card {{ border: 2px solid @accent_bg_color; }}",
        LIBRARY_ICON_SIZE, LIBRARY_ICON_CORNER_RADIUS
    ));
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let title = adw::WindowTitle::new("Leyen", "");
    let back_btn = gtk4::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Back to Library")
        .visible(false)
        .build();
    let add_menu_model = gio::Menu::new();
    add_menu_model.append(Some("Game"), Some("win.add-game"));
    add_menu_model.append(Some("Group"), Some("win.add-group"));
    let add_menu_btn = gtk4::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .menu_model(&add_menu_model)
        .tooltip_text("Add")
        .build();
    let add_game_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Game")
        .build();
    let add_button_stack = gtk4::Stack::new();
    add_button_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    add_button_stack.set_transition_duration(180);
    add_button_stack.add_named(&add_menu_btn, Some("menu"));
    add_button_stack.add_named(&add_game_btn, Some("game"));
    add_button_stack.set_visible_child_name("menu");

    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_start(&back_btn);
    let menu_model = gio::Menu::new();
    menu_model.append(Some("Running Games"), Some("win.show-running-games"));
    menu_model.append(Some("Logs"), Some("win.show-logs"));
    let menu_section = gio::Menu::new();
    menu_section.append(Some("Preferences"), Some("win.show-preferences"));
    menu_section.append(Some("Keyboard Shortcuts"), Some("win.show-shortcuts"));
    menu_section.append(Some("About Leyen"), Some("win.show-about"));
    menu_model.append_section(None, &menu_section);
    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .tooltip_text("Main Menu")
        .build();
    let search_btn = gtk4::ToggleButton::builder()
        .icon_name("edit-find-symbolic")
        .tooltip_text("Search")
        .build();

    let search_entry = gtk4::SearchEntry::builder()
        .hexpand(true)
        .placeholder_text("Search games...")
        .build();

    let search_bar = gtk4::SearchBar::builder().child(&search_entry).build();

    header.pack_end(&menu_btn);
    header.pack_end(&add_button_stack);
    header.pack_end(&search_btn);

    search_bar
        .bind_property("search-mode-enabled", &search_btn, "active")
        .sync_create()
        .bidirectional()
        .build();

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.add_top_bar(&search_bar);

    let root_list_box_primary = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    let root_list_box_secondary = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    let root_list_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(240)
        .hexpand(true)
        .build();
    root_list_stack.add_named(&root_list_box_primary, Some(LIST_PAGE_PRIMARY));
    root_list_stack.add_named(&root_list_box_secondary, Some(LIST_PAGE_SECONDARY));
    root_list_stack.set_visible_child_name(LIST_PAGE_PRIMARY);

    let group_list_box_primary = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    let group_list_box_secondary = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    let group_list_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(240)
        .hexpand(true)
        .build();
    group_list_stack.add_named(&group_list_box_primary, Some(LIST_PAGE_PRIMARY));
    group_list_stack.add_named(&group_list_box_secondary, Some(LIST_PAGE_SECONDARY));
    group_list_stack.set_visible_child_name(LIST_PAGE_PRIMARY);

    let root_empty_state = adw::StatusPage::builder()
        .icon_name("applications-games-symbolic")
        .title("No games added yet")
        .description("Add a game or create a group to organize your library.")
        .build();

    let group_empty_state = adw::StatusPage::builder()
        .icon_name("folder-symbolic")
        .title("This group is empty")
        .description("Add a game while inside the group to populate it.")
        .build();

    let root_content_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .hexpand(true)
        .vexpand(true)
        .build();
    root_content_stack.add_named(&root_empty_state, Some("empty"));
    root_content_stack.add_named(&root_list_stack, Some("list"));
    root_content_stack.set_visible_child_name("empty");

    let group_content_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .hexpand(true)
        .vexpand(true)
        .build();
    group_content_stack.add_named(&group_empty_state, Some("empty"));
    group_content_stack.add_named(&group_list_stack, Some("list"));
    group_content_stack.set_visible_child_name("empty");

    let root_clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .child(&root_content_stack)
        .build();
    let group_clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .child(&group_content_stack)
        .build();

    let root_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&root_clamp)
        .build();
    let group_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&group_clamp)
        .build();

    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeftRight)
        .transition_duration(260)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_named(&root_scroll, Some("root"));
    stack.add_named(&group_scroll, Some("group"));
    stack.set_visible_child_name("root");

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&stack));

    let download_banner = adw::Banner::builder()
        .title("Downloading umu-launcher… Please wait before starting games.")
        .revealed(
            UMU_DOWNLOADING.load(Ordering::Relaxed)
                || WINETRICKS_DOWNLOADING.load(Ordering::Relaxed),
        )
        .build();
    toolbar_view.add_top_bar(&download_banner);
    toolbar_view.set_content(Some(&toast_overlay));

    let banner_for_update = download_banner.clone();
    glib::timeout_add_seconds_local(1, move || {
        let umu_down = UMU_DOWNLOADING.load(Ordering::Relaxed);
        let wt_down = WINETRICKS_DOWNLOADING.load(Ordering::Relaxed);
        let any_down = umu_down || wt_down;

        if any_down {
            banner_for_update.set_revealed(true);
            let title = if umu_down && wt_down {
                "Downloading umu-launcher & winetricks… Please wait before starting games."
            } else if umu_down {
                "Downloading umu-launcher… Please wait before starting games."
            } else {
                "Downloading winetricks…"
            };
            banner_for_update.set_title(title);
            glib::ControlFlow::Continue
        } else {
            banner_for_update.set_revealed(false);
            glib::ControlFlow::Break
        }
    });

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Leyen")
        .default_width(MAIN_WINDOW_DEFAULT_WIDTH)
        .default_height(MAIN_WINDOW_DEFAULT_HEIGHT)
        .resizable(false)
        .content(&toolbar_view)
        .build();

    let ui = LibraryUi {
        root_list_stack,
        root_list_box_primary,
        root_list_box_secondary,
        root_list_showing_primary: Rc::new(Cell::new(true)),
        root_content_stack,
        group_list_stack,
        group_list_box_primary,
        group_list_box_secondary,
        group_list_showing_primary: Rc::new(Cell::new(true)),
        group_content_stack,
        root_running_duration_labels: Rc::new(RefCell::new(std::collections::HashMap::new())),
        root_group_running_duration_labels: Rc::new(RefCell::new(std::collections::HashMap::new())),
        group_running_duration_labels: Rc::new(RefCell::new(std::collections::HashMap::new())),
        stack,
        add_button_stack: add_button_stack.clone(),
        back_btn: back_btn.clone(),
        title,
        _search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        library_state: Rc::new(RefCell::new(Vec::new())),
        current_group_id: Rc::new(RefCell::new(None)),
    };

    search_bar.set_key_capture_widget(Some(&window));

    let debounce_token = Rc::new(Cell::new(0u64));

    let ui_c = ui.clone();
    let overlay_c = toast_overlay.clone();
    let window_c = window.clone();
    search_entry.connect_search_changed(move |_| {
        let u = ui_c.clone();
        let o = overlay_c.clone();
        let w = window_c.clone();
        let token = {
            let t = debounce_token.get();
            debounce_token.set(t.wrapping_add(1));
            t.wrapping_add(1)
        };
        let dt = debounce_token.clone();
        glib::spawn_future_local(async move {
            glib::timeout_future(std::time::Duration::from_millis(200)).await;
            if dt.get() != token {
                return;
            }
            refresh_library_view(&u, &o, &w).await;
        });
    });

    let ui_c = ui.clone();
    let overlay_c = toast_overlay.clone();
    let window_c = window.clone();
    glib::spawn_future_local(async move {
        refresh_library_view(&ui_c, &overlay_c, &window_c).await;
    });

    let ui_clone = ui.clone();
    let overlay_clone = toast_overlay.clone();
    let window_clone = window.clone();
    back_btn.connect_clicked(move |_| {
        *ui_clone.current_group_id.borrow_mut() = None;
        let ui = ui_clone.clone();
        let overlay = overlay_clone.clone();
        let window = window_clone.clone();
        glib::spawn_future_local(async move {
            refresh_library_view(&ui, &overlay, &window).await;
        });
    });

    let add_game_action = gio::SimpleAction::new("add-game", None);
    let window_clone = window.clone();
    let ui_clone = ui.clone();
    add_game_action.connect_activate(move |_, _| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        glib::spawn_future_local(async move {
            show_add_library_item_dialog(&w, &u, AddLibraryItemKind::Game).await;
        });
    });
    window.add_action(&add_game_action);

    let window_clone = window.clone();
    let ui_clone = ui.clone();
    add_game_btn.connect_clicked(move |_| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        glib::spawn_future_local(async move {
            show_add_library_item_dialog(&w, &u, AddLibraryItemKind::Game).await;
        });
    });

    let add_group_action = gio::SimpleAction::new("add-group", None);
    let window_clone = window.clone();
    let ui_clone = ui.clone();
    add_group_action.connect_activate(move |_, _| {
        let w = window_clone.clone();
        let u = ui_clone.clone();
        glib::spawn_future_local(async move {
            show_add_library_item_dialog(&w, &u, AddLibraryItemKind::Group).await;
        });
    });
    window.add_action(&add_group_action);

    let running_state_version = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let ui_refresh = ui.clone();
    let overlay_refresh = toast_overlay.clone();
    let window_refresh = window.clone();
    glib::timeout_add_seconds_local(1, move || {
        if crate::instance::check_and_clear_show_signal() {
            window_refresh.set_visible(true);
            window_refresh.present();
        }

        if !window_refresh.is_visible() && !crate::launch::is_any_game_running() {
            window_refresh.close();
            return glib::ControlFlow::Break;
        }

        let ui_refresh = ui_refresh.clone();
        let overlay_refresh = overlay_refresh.clone();
        let window_refresh = window_refresh.clone();
        let running_state_version = running_state_version.clone();

        glib::spawn_future_local(async move {
            let current_version = crate::launch::running_games_version().await;
            if current_version != running_state_version.get() {
                running_state_version.set(current_version);
                refresh_library_view(&ui_refresh, &overlay_refresh, &window_refresh).await;
            } else if current_version != 0 {
                update_running_duration_labels(&ui_refresh).await;
            }
        });
        glib::ControlFlow::Continue
    });
    let prefs_action = gio::SimpleAction::new("show-preferences", None);
    let window_clone = window.clone();
    prefs_action.connect_activate(move |_, _| {
        let window = window_clone.clone();
        glib::spawn_future_local(async move {
            show_global_settings(&window).await;
        });
    });
    window.add_action(&prefs_action);

    let logs_action = gio::SimpleAction::new("show-logs", None);
    let window_clone_logs = window.clone();
    logs_action.connect_activate(move |_, _| {
        let window = window_clone_logs.clone();
        glib::spawn_future_local(async move {
            show_log_window(&window, None).await;
        });
    });
    window.add_action(&logs_action);

    let running_games_action = gio::SimpleAction::new("show-running-games", None);
    let window_clone_running = window.clone();
    running_games_action.connect_activate(move |_, _| {
        let window = window_clone_running.clone();
        glib::spawn_future_local(async move {
            show_running_games_window(&window).await;
        });
    });
    window.add_action(&running_games_action);

    let shortcuts_action = gio::SimpleAction::new("show-shortcuts", None);
    shortcuts_action.connect_activate(move |_, _| {
        let win = gtk4::ShortcutsWindow::builder().build();
        let quit_shortcut = gtk4::ShortcutsShortcut::builder()
            .title("Quit Leyen")
            .accelerator("<Ctrl>Q")
            .build();
        let search_shortcut = gtk4::ShortcutsShortcut::builder()
            .title("Search Games")
            .accelerator("<Ctrl>F")
            .build();
        let general_group = gtk4::ShortcutsGroup::builder()
            .title("General")
            .build();
        general_group.append(&quit_shortcut);
        general_group.append(&search_shortcut);
        let general_section = gtk4::ShortcutsSection::builder()
            .title("General")
            .max_height(2)
            .build();
        general_section.append(&general_group);
        win.add_section(&general_section);
        win.present();
    });
    window.add_action(&shortcuts_action);

    let about_action = gio::SimpleAction::new("show-about", None);
    about_action.connect_activate(move |_, _| {
        let about = adw::AboutWindow::builder()
            .application_name("Leyen")
            .application_icon("com.github.sachesi.leyen")
            .version(env!("CARGO_PKG_VERSION"))
            .developer_name("sachesi")
            .website("https://github.com/sachesi/leyen")
            .issue_url("https://github.com/sachesi/leyen/issues")
            .license_type(gtk4::License::Gpl30)
            .build();
        about.present();
    });
    window.add_action(&about_action);

    let search_btn_clone = search_btn.clone();
    let search_toggle_action = gio::SimpleAction::new("toggle-search", None);
    search_toggle_action.connect_activate(move |_, _| {
        search_btn_clone.set_active(!search_btn_clone.is_active());
    });
    window.add_action(&search_toggle_action);

    window.connect_close_request(move |win| {
        if crate::launch::is_any_game_running() {
            win.set_visible(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    app.set_accels_for_action("app.quit", &["<Ctrl>Q"]);
    app.set_accels_for_action("win.toggle-search", &["<Ctrl>F"]);

    window.present();

    if crate::cli::take_open_logs_on_start() {
        glib::spawn_future_local(async move {
            show_log_window(&window, None).await;
        });
    }
}
