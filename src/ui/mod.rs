pub mod deps_dialog;
pub mod game_dialogs;
pub mod log_window;
pub mod settings;

use std::cell::RefCell;
use std::rc::Rc;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;

use crate::config::{load_games, load_settings, save_settings};
use crate::launch::launch_game;
use crate::models::{Game, ViewMode};
use crate::umu::{is_umu_run_available, UMU_DOWNLOADING};

use self::game_dialogs::{show_add_game_dialog, show_delete_confirmation, show_edit_game_dialog};
use self::log_window::show_log_window;
use self::settings::show_global_settings;

pub fn build_ui(app: &adw::Application) {
    let mut settings = load_settings();
    let view_mode_state = Rc::new(RefCell::new(settings.view_mode.clone()));

    let header = adw::HeaderBar::builder().build();

    let add_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Game")
        .build();

    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search games")
        .hexpand(true)
        .build();

    let search_clamp = adw::Clamp::builder()
        .maximum_size(400)
        .child(&search_entry)
        .build();

    let grid_toggle = gtk4::ToggleButton::builder()
        .icon_name("view-grid-symbolic")
        .tooltip_text("Grid view")
        .active(settings.view_mode == ViewMode::Grid)
        .build();

    let list_toggle = gtk4::ToggleButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("List view")
        .group(&grid_toggle)
        .active(settings.view_mode == ViewMode::List)
        .build();

    let toggle_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .css_classes(["linked"])
        .build();
    toggle_box.append(&grid_toggle);
    toggle_box.append(&list_toggle);

    let menu_model = gio::Menu::new();
    menu_model.append(Some("Preferences"), Some("win.show-preferences"));
    menu_model.append(Some("Logs"), Some("win.show-logs"));

    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .tooltip_text("Main Menu")
        .build();

    header.pack_start(&add_btn);
    header.set_title_widget(Some(&search_clamp));
    header.pack_end(&menu_btn);
    header.pack_end(&toggle_box);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);

    let clamp = adw::Clamp::builder()
        .maximum_size(1100)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(16)
        .margin_end(16)
        .build();

    let flow_box = gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .row_spacing(12)
        .column_spacing(12)
        .homogeneous(true)
        .min_children_per_line(2)
        .build();

    let list_box = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .css_classes(["boxed-list"])
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
        .label("Add a game to build your library.")
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .css_classes(["dim-label"])
        .build();

    empty_state.append(&empty_label);
    empty_state.append(&empty_hint);

    let content_stack = gtk4::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .build();
    content_stack.add_named(&flow_box, Some("grid"));
    content_stack.add_named(&list_box, Some("list"));
    content_stack.add_named(&empty_state, Some("empty"));

    clamp.set_child(Some(&content_stack));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));

    let download_banner = adw::Banner::builder()
        .title("Downloading umu-launcher… Please wait before starting games.")
        .revealed(UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed))
        .build();
    toolbar_view.add_top_bar(&download_banner);

    toolbar_view.set_content(Some(&toast_overlay));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Leyen")
        .default_width(980)
        .default_height(640)
        .content(&toolbar_view)
        .build();

    let games = load_games();
    populate_game_views(
        &flow_box,
        &list_box,
        &content_stack,
        &games,
        &search_entry.text(),
        &toast_overlay,
        &window,
        &view_mode_state.borrow(),
    );

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

    let prefs_action = gio::SimpleAction::new("show-preferences", None);
    let window_clone = window.clone();
    let overlay_for_settings = toast_overlay.clone();
    prefs_action.connect_activate(move |_, _| {
        show_global_settings(&window_clone, &overlay_for_settings);
    });
    window.add_action(&prefs_action);

    let logs_action = gio::SimpleAction::new("show-logs", None);
    let window_clone_logs = window.clone();
    logs_action.connect_activate(move |_, _| {
        show_log_window(&window_clone_logs);
    });
    window.add_action(&logs_action);

    let window_clone_2 = window.clone();
    let flow_box_clone = flow_box.clone();
    let list_box_clone = list_box.clone();
    let content_stack_clone = content_stack.clone();
    let overlay_clone = toast_overlay.clone();
    let search_entry_clone = search_entry.clone();
    let mode_clone = view_mode_state.clone();
    add_btn.connect_clicked(move |_| {
        show_add_game_dialog(
            &window_clone_2,
            &flow_box_clone,
            &list_box_clone,
            &content_stack_clone,
            &search_entry_clone,
            &overlay_clone,
            &mode_clone.borrow(),
        );
    });

    let flow_box_search = flow_box.clone();
    let list_box_search = list_box.clone();
    let stack_search = content_stack.clone();
    let overlay_search = toast_overlay.clone();
    let window_search = window.clone();
    let mode_search = view_mode_state.clone();
    search_entry.connect_search_changed(move |entry| {
        let games = load_games();
        populate_game_views(
            &flow_box_search,
            &list_box_search,
            &stack_search,
            &games,
            &entry.text(),
            &overlay_search,
            &window_search,
            &mode_search.borrow(),
        );
    });

    let flow_box_mode = flow_box.clone();
    let list_box_mode = list_box.clone();
    let stack_mode = content_stack.clone();
    let overlay_mode = toast_overlay.clone();
    let window_mode = window.clone();
    let search_mode = search_entry.clone();
    let mode_state = view_mode_state.clone();
    grid_toggle.connect_toggled(move |btn| {
        if !btn.is_active() {
            return;
        }
        *mode_state.borrow_mut() = ViewMode::Grid;
        let mut s = load_settings();
        s.view_mode = ViewMode::Grid;
        save_settings(&s);
        let games = load_games();
        populate_game_views(
            &flow_box_mode,
            &list_box_mode,
            &stack_mode,
            &games,
            &search_mode.text(),
            &overlay_mode,
            &window_mode,
            &mode_state.borrow(),
        );
    });

    let flow_box_mode2 = flow_box.clone();
    let list_box_mode2 = list_box.clone();
    let stack_mode2 = content_stack.clone();
    let overlay_mode2 = toast_overlay.clone();
    let window_mode2 = window.clone();
    let search_mode2 = search_entry.clone();
    let mode_state2 = view_mode_state.clone();
    list_toggle.connect_toggled(move |btn| {
        if !btn.is_active() {
            return;
        }
        *mode_state2.borrow_mut() = ViewMode::List;
        let mut s = load_settings();
        s.view_mode = ViewMode::List;
        save_settings(&s);
        let games = load_games();
        populate_game_views(
            &flow_box_mode2,
            &list_box_mode2,
            &stack_mode2,
            &games,
            &search_mode2.text(),
            &overlay_mode2,
            &window_mode2,
            &mode_state2.borrow(),
        );
    });

    settings.view_mode = view_mode_state.borrow().clone();
    save_settings(&settings);
    window.present();
}

#[allow(clippy::too_many_arguments)]
pub fn populate_game_views(
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    games: &[Game],
    search_text: &str,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    view_mode: &ViewMode,
) {
    while let Some(child) = flow_box.first_child() {
        flow_box.remove(&child);
    }
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let query = search_text.to_lowercase();
    let filtered: Vec<Game> = games
        .iter()
        .filter(|g| {
            query.is_empty()
                || g.title.to_lowercase().contains(&query)
                || g.exe_path.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        stack.set_visible_child_name("empty");
        return;
    }

    for game in &filtered {
        flow_box.insert(
            &build_grid_card(
                game,
                overlay,
                window,
                flow_box,
                list_box,
                stack,
                search_text,
                view_mode,
            ),
            -1,
        );
        list_box.append(&build_list_row(
            game,
            overlay,
            window,
            flow_box,
            list_box,
            stack,
            search_text,
            view_mode,
        ));
    }

    match view_mode {
        ViewMode::Grid => stack.set_visible_child_name("grid"),
        ViewMode::List => stack.set_visible_child_name("list"),
    }
}

fn build_grid_card(
    game: &Game,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_text: &str,
    view_mode: &ViewMode,
) -> gtk4::FlowBoxChild {
    let child = gtk4::FlowBoxChild::new();

    let button = gtk4::Button::builder().has_frame(false).build();
    button.add_css_class("game-card");
    button.set_size_request(220, 130);

    let root = gtk4::Overlay::new();

    if let Some(path) = &game.cover_path {
        let cover = gtk4::Picture::for_filename(path);
        cover.set_can_shrink(true);
        cover.set_content_fit(gtk4::ContentFit::Cover);
        root.set_child(Some(&cover));
    } else {
        let placeholder = gtk4::Box::builder()
            .css_classes(["game-card-placeholder"])
            .halign(gtk4::Align::Fill)
            .valign(gtk4::Align::Fill)
            .build();

        let initials: String = game
            .title
            .split_whitespace()
            .take(2)
            .filter_map(|part| part.chars().next())
            .collect();
        let label = gtk4::Label::builder()
            .label(if initials.is_empty() { "?" } else { &initials })
            .css_classes(["title-1"])
            .build();
        placeholder.append(&label);
        root.set_child(Some(&placeholder));
    }

    let title_bar = gtk4::Box::builder()
        .css_classes(["game-card-title-bar"])
        .valign(gtk4::Align::End)
        .halign(gtk4::Align::Fill)
        .build();

    let title = gtk4::Label::builder()
        .label(&game.title)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .css_classes(["game-card-title"])
        .build();
    title_bar.append(&title);
    root.add_overlay(&title_bar);

    let menu_model = gio::Menu::new();
    menu_model.append(Some("Launch"), Some("card.launch"));
    menu_model.append(Some("Edit"), Some("card.edit"));
    menu_model.append(Some("Delete"), Some("card.delete"));

    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .valign(gtk4::Align::Start)
        .halign(gtk4::Align::End)
        .margin_top(8)
        .margin_end(8)
        .menu_model(&menu_model)
        .css_classes(["flat", "game-card-menu"])
        .build();
    root.add_overlay(&menu_btn);

    let action_group = gio::SimpleActionGroup::new();

    let launch_action = gio::SimpleAction::new("launch", None);
    let game_for_launch = game.clone();
    let overlay_launch = overlay.clone();
    launch_action.connect_activate(move |_, _| {
        launch_game(&game_for_launch, &overlay_launch);
    });

    let search_text = search_text.to_string();
    let current_mode = view_mode.clone();

    let edit_action = gio::SimpleAction::new("edit", None);
    let search_text_edit = search_text.clone();
    let current_mode_edit = current_mode.clone();
    let game_for_edit = game.clone();
    let flow_edit = flow_box.clone();
    let list_edit = list_box.clone();
    let stack_edit = stack.clone();
    let window_edit = window.clone();
    let overlay_edit = overlay.clone();
    edit_action.connect_activate(move |_, _| {
        let search_entry = gtk4::SearchEntry::builder().text(&search_text_edit).build();
        show_edit_game_dialog(
            &window_edit,
            &flow_edit,
            &list_edit,
            &stack_edit,
            &search_entry,
            &overlay_edit,
            &current_mode_edit,
            &game_for_edit,
        );
    });

    let delete_action = gio::SimpleAction::new("delete", None);
    let search_text_delete = search_text.clone();
    let current_mode_delete = current_mode.clone();
    let game_id = game.id.clone();
    let flow_delete = flow_box.clone();
    let list_delete = list_box.clone();
    let stack_delete = stack.clone();
    let window_delete = window.clone();
    let overlay_delete = overlay.clone();
    delete_action.connect_activate(move |_, _| {
        let search_entry = gtk4::SearchEntry::builder()
            .text(&search_text_delete)
            .build();
        show_delete_confirmation(
            &window_delete,
            &flow_delete,
            &list_delete,
            &stack_delete,
            &search_entry,
            &overlay_delete,
            &current_mode_delete,
            &game_id,
        );
    });

    action_group.add_action(&launch_action);
    action_group.add_action(&edit_action);
    action_group.add_action(&delete_action);
    menu_btn.insert_action_group("card", Some(&action_group));

    let game_launch = game.clone();
    let overlay_launch = overlay.clone();
    button.connect_clicked(move |_| {
        launch_game(&game_launch, &overlay_launch);
    });

    button.set_child(Some(&root));
    child.set_child(Some(&button));
    child
}

fn build_list_row(
    game: &Game,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_text: &str,
    view_mode: &ViewMode,
) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .build();

    let thumb = if let Some(path) = &game.cover_path {
        gtk4::Picture::for_filename(path)
    } else {
        gtk4::Picture::for_icon_name("image-missing-symbolic")
    };
    thumb.set_size_request(48, 48);
    thumb.set_content_fit(gtk4::ContentFit::Cover);

    let labels = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();

    let title = gtk4::Label::builder()
        .label(&game.title)
        .xalign(0.0)
        .css_classes(["title-5"])
        .build();
    let subtitle = gtk4::Label::builder()
        .label(&game.exe_path)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .css_classes(["dim-label"])
        .build();
    labels.append(&title);
    labels.append(&subtitle);

    let menu_model = gio::Menu::new();
    menu_model.append(Some("Edit"), Some("row.edit"));
    menu_model.append(Some("Delete"), Some("row.delete"));

    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu_model)
        .build();

    let play_btn = gtk4::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .css_classes(["suggested-action", "circular"])
        .build();

    let action_group = gio::SimpleActionGroup::new();
    let search_text = search_text.to_string();
    let current_mode = view_mode.clone();

    let edit_action = gio::SimpleAction::new("edit", None);
    let search_text_edit = search_text.clone();
    let current_mode_edit = current_mode.clone();
    let game_for_edit = game.clone();
    let flow_edit = flow_box.clone();
    let list_edit = list_box.clone();
    let stack_edit = stack.clone();
    let window_edit = window.clone();
    let overlay_edit = overlay.clone();
    edit_action.connect_activate(move |_, _| {
        let search_entry = gtk4::SearchEntry::builder().text(&search_text_edit).build();
        show_edit_game_dialog(
            &window_edit,
            &flow_edit,
            &list_edit,
            &stack_edit,
            &search_entry,
            &overlay_edit,
            &current_mode_edit,
            &game_for_edit,
        );
    });

    let delete_action = gio::SimpleAction::new("delete", None);
    let search_text_delete = search_text.clone();
    let current_mode_delete = current_mode.clone();
    let game_id = game.id.clone();
    let flow_delete = flow_box.clone();
    let list_delete = list_box.clone();
    let stack_delete = stack.clone();
    let window_delete = window.clone();
    let overlay_delete = overlay.clone();
    delete_action.connect_activate(move |_, _| {
        let search_entry = gtk4::SearchEntry::builder()
            .text(&search_text_delete)
            .build();
        show_delete_confirmation(
            &window_delete,
            &flow_delete,
            &list_delete,
            &stack_delete,
            &search_entry,
            &overlay_delete,
            &current_mode_delete,
            &game_id,
        );
    });

    action_group.add_action(&edit_action);
    action_group.add_action(&delete_action);
    menu_btn.insert_action_group("row", Some(&action_group));

    let game_launch = game.clone();
    let overlay_launch = overlay.clone();
    play_btn.connect_clicked(move |_| {
        launch_game(&game_launch, &overlay_launch);
    });

    content.append(&thumb);
    content.append(&labels);
    content.append(&menu_btn);
    content.append(&play_btn);
    row.set_child(Some(&content));
    row
}
