pub mod deps_dialog;
pub mod game_dialogs;
pub mod log_window;
pub mod running_games;
pub mod settings;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;

use crate::config::load_library;
use crate::launch::{
    has_running_games, is_game_running, launch_game, running_game_elapsed, running_games_version,
    stop_game,
};
use crate::models::{Game, GameGroup, LibraryItem};
use crate::umu::{UMU_DOWNLOADING, is_umu_run_available};

use self::game_dialogs::{
    AddLibraryItemKind, show_add_library_item_dialog, show_delete_confirmation,
    show_edit_game_dialog, show_edit_group_dialog,
};
use self::log_window::show_log_window;
use self::running_games::show_running_games_window;
use self::settings::show_global_settings;

#[derive(Clone)]
pub struct LibraryUi {
    pub root_list_box: gtk4::Box,
    pub root_empty_state: gtk4::Box,
    pub group_list_box: gtk4::Box,
    pub group_empty_state: gtk4::Box,
    pub stack: gtk4::Stack,
    pub add_button_stack: gtk4::Stack,
    pub back_btn: gtk4::Button,
    pub title: adw::WindowTitle,
    pub library_state: Rc<RefCell<Vec<LibraryItem>>>,
    pub current_group_id: Rc<RefCell<Option<String>>>,
}

fn format_playtime(playtime_seconds: u64) -> String {
    let hours = playtime_seconds / 3600;
    let minutes = (playtime_seconds % 3600) / 60;

    if hours > 0 {
        format!("Playtime: {}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("Playtime: {}m", minutes)
    } else {
        format!("Playtime: {}s", playtime_seconds)
    }
}

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

fn format_last_played(epoch_seconds: u64) -> String {
    if epoch_seconds == 0 {
        return "Last played: never".to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(epoch_seconds);
    let delta = now.saturating_sub(epoch_seconds);

    let ago = if delta < 60 {
        format!("{}s ago", delta)
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    };

    format!("Last played: {}", ago)
}

fn display_proton_name(proton: &str) -> String {
    if proton.trim().is_empty() || proton == "Default" {
        return "Default Proton".to_string();
    }

    PathBuf::from(proton)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| proton.to_string())
}

fn format_group_defaults(group: &GameGroup) -> Option<String> {
    let mut parts = Vec::new();

    if !group.defaults.prefix_path.trim().is_empty() {
        parts.push("Prefix".to_string());
    }

    if !group.defaults.proton.trim().is_empty() && group.defaults.proton != "Default" {
        parts.push(display_proton_name(&group.defaults.proton));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

fn handle_game_primary_action(game: &Game, overlay: &adw::ToastOverlay) {
    if is_game_running(&game.id) {
        match stop_game(&game.id) {
            Ok(true) => {
                overlay.add_toast(adw::Toast::new(&format!("Stopping {}...", game.title)));
            }
            Ok(false) => overlay.add_toast(adw::Toast::new("Game is no longer running")),
            Err(err) => {
                overlay.add_toast(adw::Toast::new(&format!("Failed to stop game: {}", err)));
            }
        }
    } else {
        launch_game(game, overlay);
    }
}

fn find_group<'a>(items: &'a [LibraryItem], group_id: &str) -> Option<&'a GameGroup> {
    items.iter().find_map(|item| match item {
        LibraryItem::Group(group) if group.id == group_id => Some(group),
        _ => None,
    })
}

fn group_last_played(group: &GameGroup) -> u64 {
    group
        .games
        .iter()
        .map(|game| game.last_played_epoch_seconds)
        .max()
        .unwrap_or(0)
}

fn group_has_running_games(group: &GameGroup) -> bool {
    group.games.iter().any(|game| is_game_running(&game.id))
}

fn update_add_button_mode(ui: &LibraryUi) {
    let child_name = if ui.current_group_id.borrow().is_some() {
        "game"
    } else {
        "menu"
    };
    ui.add_button_stack.set_visible_child_name(child_name);
}

fn open_group(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    group_id: &str,
) {
    *ui.current_group_id.borrow_mut() = Some(group_id.to_string());
    refresh_library_view(ui, overlay, window);
}

pub fn refresh_library_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    *ui.library_state.borrow_mut() = load_library();
    populate_root_view(ui, overlay, window);
    populate_group_view(ui, overlay, window);

    if let Some(group_id) = ui.current_group_id.borrow().clone() {
        if find_group(&ui.library_state.borrow(), &group_id).is_none() {
            *ui.current_group_id.borrow_mut() = None;
            ui.stack.set_visible_child_name("root");
            ui.back_btn.set_visible(false);
        } else {
            ui.stack.set_visible_child_name("group");
            ui.back_btn.set_visible(true);
        }
    }

    if ui.current_group_id.borrow().is_none() {
        ui.stack.set_visible_child_name("root");
        ui.back_btn.set_visible(false);
        ui.title.set_title("Leyen");
        ui.title.set_subtitle("");
    }

    update_add_button_mode(ui);
}

fn populate_root_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    while let Some(child) = ui.root_list_box.first_child() {
        ui.root_list_box.remove(&child);
    }

    let items = ui.library_state.borrow();
    if items.is_empty() {
        ui.root_list_box.append(&ui.root_empty_state);
        return;
    }

    let mut sorted_items: Vec<LibraryItem> = items.clone();
    sorted_items.sort_by(|left, right| {
        let left_kind_rank = match left {
            LibraryItem::Game(_) => 0,
            LibraryItem::Group(_) => 1,
        };
        let right_kind_rank = match right {
            LibraryItem::Game(_) => 0,
            LibraryItem::Group(_) => 1,
        };

        let left_running = match left {
            LibraryItem::Game(game) => is_game_running(&game.id),
            LibraryItem::Group(group) => group_has_running_games(group),
        };
        let right_running = match right {
            LibraryItem::Game(game) => is_game_running(&game.id),
            LibraryItem::Group(group) => group_has_running_games(group),
        };

        let left_last_played = match left {
            LibraryItem::Game(game) => game.last_played_epoch_seconds,
            LibraryItem::Group(group) => group_last_played(group),
        };
        let right_last_played = match right {
            LibraryItem::Game(game) => game.last_played_epoch_seconds,
            LibraryItem::Group(group) => group_last_played(group),
        };

        left_kind_rank
            .cmp(&right_kind_rank)
            .then_with(|| right_running.cmp(&left_running))
            .then_with(|| right_last_played.cmp(&left_last_played))
            .then_with(|| {
                left.title()
                    .to_lowercase()
                    .cmp(&right.title().to_lowercase())
            })
    });

    for item in &sorted_items {
        match item {
            LibraryItem::Game(game) => {
                ui.root_list_box
                    .append(&build_game_card(game, overlay, window, ui));
            }
            LibraryItem::Group(group) => {
                ui.root_list_box
                    .append(&build_group_card(group, overlay, window, ui));
            }
        }
    }
}

fn populate_group_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    while let Some(child) = ui.group_list_box.first_child() {
        ui.group_list_box.remove(&child);
    }

    let Some(group_id) = ui.current_group_id.borrow().clone() else {
        return;
    };
    let items = ui.library_state.borrow();
    let Some(group) = find_group(&items, &group_id).cloned() else {
        return;
    };

    ui.title.set_title(&group.title);
    ui.title.set_subtitle("Group");

    if group.games.is_empty() {
        ui.group_list_box.append(&ui.group_empty_state);
        return;
    }

    let mut games = group.games.clone();
    games.sort_by(|left, right| {
        is_game_running(&right.id)
            .cmp(&is_game_running(&left.id))
            .then_with(|| {
                right
                    .last_played_epoch_seconds
                    .cmp(&left.last_played_epoch_seconds)
            })
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });

    for game in &games {
        ui.group_list_box
            .append(&build_game_card(game, overlay, window, ui));
    }
}

fn build_group_card(
    group: &GameGroup,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    ui: &LibraryUi,
) -> gtk4::Frame {
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

    let icon = gtk4::Image::builder()
        .icon_name("folder-symbolic")
        .pixel_size(48)
        .valign(gtk4::Align::Start)
        .build();

    let info_column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();

    let title_label = gtk4::Label::builder()
        .label(&group.title)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();
    let count_label = gtk4::Label::builder()
        .label(format!(
            "{} game{}",
            group.games.len(),
            if group.games.len() == 1 { "" } else { "s" }
        ))
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    let last_played_label = gtk4::Label::builder()
        .label(format_last_played(group_last_played(group)))
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();

    info_column.append(&title_label);
    info_column.append(&count_label);
    info_column.append(&last_played_label);
    if let Some(defaults_label) = format_group_defaults(group) {
        info_column.append(
            &gtk4::Label::builder()
                .label(defaults_label)
                .xalign(0.0)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }

    if group_has_running_games(group) {
        let running_count = group
            .games
            .iter()
            .filter(|game| is_game_running(&game.id))
            .count();
        let running_label = gtk4::Label::builder()
            .label(format!("{} running", running_count))
            .xalign(0.0)
            .css_classes(["caption", "accent"])
            .build();
        info_column.append(&running_label);
    }

    let open_area = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    open_area.append(&icon);
    open_area.append(&info_column);

    let button_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Center)
        .build();

    let edit_btn = gtk4::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Edit Group")
        .build();
    let delete_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete Group")
        .css_classes(["destructive-action"])
        .build();
    let open_btn = gtk4::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Open Group")
        .css_classes(["suggested-action", "circular"])
        .build();

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    open_btn
        .connect_clicked(move |_| open_group(&ui_clone, &overlay_clone, &window_clone, &group_id));

    let gesture = gtk4::GestureClick::new();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    gesture.connect_pressed(move |_, _, _, _| {
        open_group(&ui_clone, &overlay_clone, &window_clone, &group_id);
    });
    open_area.add_controller(gesture);

    let group_clone = group.clone();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    edit_btn.connect_clicked(move |_| {
        show_edit_group_dialog(&window_clone, &ui_clone, &overlay_clone, &group_clone);
    });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let group_id = group.id.clone();
    delete_btn.connect_clicked(move |_| {
        show_delete_confirmation(&window_clone, &ui_clone, &overlay_clone, &group_id);
    });

    button_box.append(&edit_btn);
    button_box.append(&delete_btn);
    button_box.append(&open_btn);

    content.append(&open_area);
    content.append(&button_box);
    card.set_child(Some(&content));
    card
}

fn build_game_card(
    game: &Game,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    ui: &LibraryUi,
) -> gtk4::Frame {
    let game_running = is_game_running(&game.id);
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

    let icon = gtk4::Image::builder()
        .icon_name("application-x-executable-symbolic")
        .pixel_size(48)
        .valign(gtk4::Align::Start)
        .build();

    let info_column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    let open_area = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();

    let title_label = gtk4::Label::builder()
        .label(&game.title)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();

    info_column.append(&title_label);
    info_column.append(
        &gtk4::Label::builder()
            .label(format_playtime(game.playtime_seconds))
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    info_column.append(
        &gtk4::Label::builder()
            .label(format_last_played(game.last_played_epoch_seconds))
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );

    if game_running {
        info_column.append(
            &gtk4::Label::builder()
                .label(format!(
                    "Running for {}",
                    format_duration_brief(
                        running_game_elapsed(&game.id)
                            .map(|elapsed| elapsed.as_secs())
                            .unwrap_or(0)
                    )
                ))
                .xalign(0.0)
                .css_classes(["caption", "accent"])
                .build(),
        );
    }

    open_area.append(&icon);
    open_area.append(&info_column);

    let gesture = gtk4::GestureClick::new();
    let game_clone = game.clone();
    let overlay_clone = overlay.clone();
    gesture.connect_pressed(move |_, _, _, _| {
        handle_game_primary_action(&game_clone, &overlay_clone);
    });
    open_area.add_controller(gesture);

    let button_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Center)
        .build();

    let edit_btn = gtk4::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Edit Game")
        .build();
    let delete_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete Game")
        .css_classes(["destructive-action"])
        .build();
    let play_btn = gtk4::Button::builder()
        .icon_name(if game_running {
            "media-playback-stop-symbolic"
        } else {
            "media-playback-start-symbolic"
        })
        .css_classes(if game_running {
            ["destructive-action", "circular"]
        } else {
            ["suggested-action", "circular"]
        })
        .tooltip_text(if game_running {
            "Stop Game"
        } else {
            "Launch Game"
        })
        .build();

    let game_clone = game.clone();
    let overlay_clone = overlay.clone();
    play_btn.connect_clicked(move |_| {
        handle_game_primary_action(&game_clone, &overlay_clone);
    });

    let game_clone = game.clone();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    edit_btn.connect_clicked(move |_| {
        show_edit_game_dialog(&window_clone, &ui_clone, &overlay_clone, &game_clone);
    });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let window_clone = window.clone();
    let game_id = game.id.clone();
    delete_btn.connect_clicked(move |_| {
        show_delete_confirmation(&window_clone, &ui_clone, &overlay_clone, &game_id);
    });

    button_box.append(&edit_btn);
    button_box.append(&delete_btn);
    button_box.append(&play_btn);

    content.append(&open_area);
    content.append(&button_box);
    card.set_child(Some(&content));
    card
}

pub fn build_ui(app: &adw::Application) {
    let css = gtk4::CssProvider::new();
    css.load_from_string(
        "image.edit-icon { min-width: 0px; min-height: 0px; \
         margin: 0px; padding: 0px; opacity: 0; }",
    );
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
    add_button_stack.add_named(&add_menu_btn, Some("menu"));
    add_button_stack.add_named(&add_game_btn, Some("game"));
    add_button_stack.set_visible_child_name("menu");

    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_start(&back_btn);
    let menu_model = gio::Menu::new();
    menu_model.append(Some("Preferences"), Some("win.show-preferences"));
    menu_model.append(Some("Running Games"), Some("win.show-running-games"));
    menu_model.append(Some("Logs"), Some("win.show-logs"));
    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .tooltip_text("Main Menu")
        .build();
    header.pack_end(&menu_btn);
    header.pack_end(&add_button_stack);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);

    let root_list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    let group_list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();

    let root_empty_state = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .spacing(6)
        .build();
    root_empty_state.append(
        &gtk4::Label::builder()
            .label("No games added yet")
            .css_classes(["title-3"])
            .build(),
    );
    root_empty_state.append(
        &gtk4::Label::builder()
            .label("Add a game or create a group to organize your library.")
            .wrap(true)
            .justify(gtk4::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );

    let group_empty_state = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .spacing(6)
        .build();
    group_empty_state.append(
        &gtk4::Label::builder()
            .label("This group is empty")
            .css_classes(["title-3"])
            .build(),
    );
    group_empty_state.append(
        &gtk4::Label::builder()
            .label("Add a game while inside the group to populate it.")
            .wrap(true)
            .justify(gtk4::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );

    let root_clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .child(&root_list_box)
        .build();
    let group_clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .child(&group_list_box)
        .build();

    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeftRight)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_named(&root_clamp, Some("root"));
    stack.add_named(&group_clamp, Some("group"));
    stack.set_visible_child_name("root");

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&stack)
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
        .default_width(540)
        .default_height(640)
        .resizable(false)
        .content(&toolbar_view)
        .build();

    let ui = LibraryUi {
        root_list_box,
        root_empty_state,
        group_list_box,
        group_empty_state,
        stack,
        add_button_stack: add_button_stack.clone(),
        back_btn: back_btn.clone(),
        title,
        library_state: Rc::new(RefCell::new(load_library())),
        current_group_id: Rc::new(RefCell::new(None)),
    };

    refresh_library_view(&ui, &toast_overlay, &window);

    let ui_clone = ui.clone();
    let overlay_clone = toast_overlay.clone();
    let window_clone = window.clone();
    back_btn.connect_clicked(move |_| {
        *ui_clone.current_group_id.borrow_mut() = None;
        refresh_library_view(&ui_clone, &overlay_clone, &window_clone);
    });

    let add_game_action = gio::SimpleAction::new("add-game", None);
    let window_clone = window.clone();
    let ui_clone = ui.clone();
    let overlay_clone = toast_overlay.clone();
    add_game_action.connect_activate(move |_, _| {
        show_add_library_item_dialog(
            &window_clone,
            &ui_clone,
            &overlay_clone,
            AddLibraryItemKind::Game,
        );
    });
    window.add_action(&add_game_action);

    let window_clone = window.clone();
    let ui_clone = ui.clone();
    let overlay_clone = toast_overlay.clone();
    add_game_btn.connect_clicked(move |_| {
        show_add_library_item_dialog(
            &window_clone,
            &ui_clone,
            &overlay_clone,
            AddLibraryItemKind::Game,
        );
    });

    let add_group_action = gio::SimpleAction::new("add-group", None);
    let window_clone = window.clone();
    let ui_clone = ui.clone();
    let overlay_clone = toast_overlay.clone();
    add_group_action.connect_activate(move |_, _| {
        show_add_library_item_dialog(
            &window_clone,
            &ui_clone,
            &overlay_clone,
            AddLibraryItemKind::Group,
        );
    });
    window.add_action(&add_group_action);

    let running_state_version = std::cell::Cell::new(running_games_version());
    let ui_refresh = ui.clone();
    let overlay_refresh = toast_overlay.clone();
    let window_refresh = window.clone();
    glib::timeout_add_seconds_local(1, move || {
        let current_version = running_games_version();
        if current_version != running_state_version.get() || has_running_games() {
            running_state_version.set(current_version);
            refresh_library_view(&ui_refresh, &overlay_refresh, &window_refresh);
        }
        glib::ControlFlow::Continue
    });

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
        show_log_window(&window_clone_logs, None);
    });
    window.add_action(&logs_action);

    let running_games_action = gio::SimpleAction::new("show-running-games", None);
    let window_clone_running = window.clone();
    running_games_action.connect_activate(move |_, _| {
        show_running_games_window(&window_clone_running);
    });
    window.add_action(&running_games_action);

    window.present();
}
