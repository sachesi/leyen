pub mod deps_dialog;
pub mod game_dialogs;
pub mod log_window;
pub mod running_games;
pub mod settings;

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::path::Path;
use std::rc::Rc;

use image::imageops::FilterType;
use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::Cast;

use crate::config::load_library;
use crate::icons::{game_icon_file, group_icon_file};
use crate::launch::{launch_game, running_games_snapshot, running_games_version, stop_game};
use crate::models::{Game, GameGroup, LibraryItem};
use crate::umu::{UMU_DOWNLOADING, is_umu_run_available};

use self::game_dialogs::{
    AddLibraryItemKind, show_add_library_item_dialog, show_delete_confirmation,
    show_edit_game_dialog, show_edit_group_dialog,
};
use self::log_window::show_log_window;
use self::running_games::show_running_games_window;
use self::settings::show_global_settings;

pub(crate) const MAIN_WINDOW_DEFAULT_WIDTH: i32 = 540;
pub(crate) const MAIN_WINDOW_DEFAULT_HEIGHT: i32 = 640;
pub(crate) const SECONDARY_WINDOW_DEFAULT_WIDTH: i32 = MAIN_WINDOW_DEFAULT_WIDTH - 20;
pub(crate) const SECONDARY_WINDOW_DEFAULT_HEIGHT: i32 = MAIN_WINDOW_DEFAULT_HEIGHT - 20;
const LIBRARY_ICON_SIZE: i32 = 48;
const LIBRARY_ICON_CORNER_RADIUS: i32 = 7;

#[derive(Clone)]
pub struct LibraryUi {
    pub root_list_stack: gtk4::Stack,
    pub root_list_box_primary: gtk4::Box,
    pub root_list_box_secondary: gtk4::Box,
    pub root_list_showing_primary: Rc<Cell<bool>>,
    pub root_content_stack: gtk4::Stack,
    pub group_list_stack: gtk4::Stack,
    pub group_list_box_primary: gtk4::Box,
    pub group_list_box_secondary: gtk4::Box,
    pub group_list_showing_primary: Rc<Cell<bool>>,
    pub group_content_stack: gtk4::Stack,
    pub root_running_duration_labels: Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
    pub root_group_running_duration_labels:
        Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
    pub group_running_duration_labels: Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
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

type RunningGameMap = std::collections::HashMap<String, crate::launch::RunningGameSnapshot>;

const LIST_PAGE_PRIMARY: &str = "primary";
const LIST_PAGE_SECONDARY: &str = "secondary";

fn running_game_map() -> RunningGameMap {
    running_games_snapshot()
        .into_iter()
        .map(|snapshot| (snapshot.game_id.clone(), snapshot))
        .collect()
}

fn game_is_running(running_games: &RunningGameMap, game_id: &str) -> bool {
    running_games.contains_key(game_id)
}

fn title_cmp(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

fn game_display_cmp(left: &Game, right: &Game, running_games: &RunningGameMap) -> Ordering {
    game_is_running(running_games, &right.id)
        .cmp(&game_is_running(running_games, &left.id))
        .then_with(|| title_cmp(&left.title, &right.title))
}

fn root_library_item_cmp(
    left: &LibraryItem,
    right: &LibraryItem,
    running_games: &RunningGameMap,
) -> Ordering {
    let left_running_game = matches!(
        left,
        LibraryItem::Game(game) if game_is_running(running_games, &game.id)
    );
    let right_running_game = matches!(
        right,
        LibraryItem::Game(game) if game_is_running(running_games, &game.id)
    );

    right_running_game
        .cmp(&left_running_game)
        .then_with(|| match (left, right) {
            (LibraryItem::Group(left_group), LibraryItem::Group(right_group)) => {
                title_cmp(&left_group.title, &right_group.title)
            }
            (LibraryItem::Group(_), LibraryItem::Game(_)) => Ordering::Less,
            (LibraryItem::Game(_), LibraryItem::Group(_)) => Ordering::Greater,
            (LibraryItem::Game(left_game), LibraryItem::Game(right_game)) => {
                game_display_cmp(left_game, right_game, running_games)
            }
        })
}

fn clear_list_box(list_box: &gtk4::Box) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
}

fn next_swap_list_box<'a>(
    primary: &'a gtk4::Box,
    secondary: &'a gtk4::Box,
    showing_primary: &Cell<bool>,
) -> (&'a gtk4::Box, &'static str) {
    if showing_primary.get() {
        (secondary, LIST_PAGE_SECONDARY)
    } else {
        (primary, LIST_PAGE_PRIMARY)
    }
}

fn finish_list_swap(list_stack: &gtk4::Stack, showing_primary: &Cell<bool>, visible_page: &str) {
    list_stack.set_visible_child_name(visible_page);
    showing_primary.set(visible_page == LIST_PAGE_PRIMARY);
}

pub(super) fn build_library_icon(
    icon_path: Option<std::path::PathBuf>,
    fallback_icon: &str,
    valign: gtk4::Align,
) -> gtk4::Widget {
    let wrapper = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .halign(gtk4::Align::Center)
        .valign(valign)
        .build();
    wrapper.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    wrapper.set_overflow(gtk4::Overflow::Hidden);
    wrapper.add_css_class("library-icon-frame");

    if let Some(path) = icon_path.as_deref().filter(|path| path.is_file())
        && let Some(icon) = build_scaled_art_icon(path)
    {
        wrapper.append(&icon);
        return wrapper.upcast();
    }

    if fallback_icon == "folder"
        && let Some(icon) = build_themed_folder_icon()
    {
        wrapper.append(&icon);
        return wrapper.upcast();
    }

    wrapper.append(
        &gtk4::Image::builder()
            .icon_name(fallback_icon)
            .pixel_size(LIBRARY_ICON_SIZE)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .build(),
    );
    wrapper.upcast()
}

fn build_scaled_art_icon(path: &Path) -> Option<gtk4::Picture> {
    let image = image::open(path).ok()?;
    let image = crop_transparent_padding(image);
    let image = image.resize(
        (LIBRARY_ICON_SIZE * 2) as u32,
        (LIBRARY_ICON_SIZE * 2) as u32,
        FilterType::Lanczos3,
    );
    let rgba = image.to_rgba8();
    let width = i32::try_from(rgba.width()).ok()?;
    let height = i32::try_from(rgba.height()).ok()?;
    let stride = usize::try_from(width).ok()?.checked_mul(4)?;
    let bytes = gtk4::glib::Bytes::from_owned(rgba.into_raw());
    let texture = gtk4::gdk::MemoryTexture::new(
        width,
        height,
        gtk4::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );

    let picture = gtk4::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.add_css_class("library-icon-media");

    Some(picture)
}

fn build_themed_folder_icon() -> Option<gtk4::Picture> {
    let display = gtk4::gdk::Display::default()?;
    let theme = gtk4::IconTheme::for_display(&display);
    let icon = theme.lookup_icon(
        "folder",
        &[],
        LIBRARY_ICON_SIZE * 2,
        1,
        gtk4::TextDirection::Ltr,
        gtk4::IconLookupFlags::empty(),
    );

    let picture = gtk4::Picture::for_paintable(&icon);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.add_css_class("library-icon-media");

    Some(picture)
}

fn crop_transparent_padding(image: image::DynamicImage) -> image::DynamicImage {
    let Some((left, top, right, bottom)) = alpha_bounds(&image) else {
        return image;
    };

    if left == 0 && top == 0 && right + 1 == image.width() && bottom + 1 == image.height() {
        return image;
    }

    image.crop_imm(left, top, right - left + 1, bottom - top + 1)
}

fn alpha_bounds(image: &image::DynamicImage) -> Option<(u32, u32, u32, u32)> {
    let rgba = image.to_rgba8();
    let mut left = image.width();
    let mut top = image.height();
    let mut right = 0;
    let mut bottom = 0;
    let mut found = false;

    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel.0[3] <= 8 {
            continue;
        }
        found = true;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }

    found.then_some((left, top, right, bottom))
}

fn running_game_elapsed_seconds(running_games: &RunningGameMap, game_id: &str) -> Option<u64> {
    running_games
        .get(game_id)
        .map(|snapshot| snapshot.elapsed_seconds)
}

fn group_running_elapsed_seconds(group: &GameGroup, running_games: &RunningGameMap) -> Option<u64> {
    group
        .games
        .iter()
        .filter_map(|game| running_game_elapsed_seconds(running_games, &game.id))
        .max()
}

fn update_running_duration_labels(ui: &LibraryUi) {
    let snapshots = running_game_map();
    if ui.current_group_id.borrow().is_some() {
        for (game_id, label) in ui.group_running_duration_labels.borrow().iter() {
            if let Some(snapshot) = snapshots.get(game_id) {
                label.set_label(&format!(
                    "Running for {}",
                    format_duration_brief(snapshot.elapsed_seconds)
                ));
            }
        }
    } else {
        for (game_id, label) in ui.root_running_duration_labels.borrow().iter() {
            if let Some(snapshot) = snapshots.get(game_id) {
                label.set_label(&format!(
                    "Running for {}",
                    format_duration_brief(snapshot.elapsed_seconds)
                ));
            }
        }

        let items = ui.library_state.borrow();
        for item in items.iter() {
            if let LibraryItem::Group(group) = item
                && let Some(elapsed_seconds) = group_running_elapsed_seconds(group, &snapshots)
                && let Some(label) = ui
                    .root_group_running_duration_labels
                    .borrow()
                    .get(&group.id)
            {
                label.set_label(&format!(
                    "Running for {}",
                    format_duration_brief(elapsed_seconds)
                ));
            }
        }
    }
}

fn handle_game_primary_action(game: &Game, overlay: &adw::ToastOverlay) {
    if game_is_running(&running_game_map(), &game.id) {
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
    let (root_list_box, visible_page) = next_swap_list_box(
        &ui.root_list_box_primary,
        &ui.root_list_box_secondary,
        ui.root_list_showing_primary.as_ref(),
    );
    clear_list_box(root_list_box);
    ui.root_running_duration_labels.borrow_mut().clear();
    ui.root_group_running_duration_labels.borrow_mut().clear();

    let items = ui.library_state.borrow();
    if items.is_empty() {
        ui.root_content_stack.set_visible_child_name("empty");
        return;
    }

    let running_games = running_game_map();
    let mut sorted_items: Vec<LibraryItem> = items.clone();
    sorted_items.sort_by(|left, right| root_library_item_cmp(left, right, &running_games));

    for item in &sorted_items {
        match item {
            LibraryItem::Game(game) => {
                root_list_box.append(&build_game_card(
                    game,
                    overlay,
                    window,
                    ui,
                    &running_games,
                    &ui.root_running_duration_labels,
                ));
            }
            LibraryItem::Group(group) => {
                root_list_box.append(&build_group_card(
                    group,
                    overlay,
                    window,
                    ui,
                    &running_games,
                    &ui.root_group_running_duration_labels,
                ));
            }
        }
    }

    finish_list_swap(
        &ui.root_list_stack,
        ui.root_list_showing_primary.as_ref(),
        visible_page,
    );
    ui.root_content_stack.set_visible_child_name("list");
}

fn populate_group_view(
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    let (group_list_box, visible_page) = next_swap_list_box(
        &ui.group_list_box_primary,
        &ui.group_list_box_secondary,
        ui.group_list_showing_primary.as_ref(),
    );
    clear_list_box(group_list_box);
    ui.group_running_duration_labels.borrow_mut().clear();

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
        ui.group_content_stack.set_visible_child_name("empty");
        return;
    }

    let running_games = running_game_map();
    let mut games = group.games.clone();
    games.sort_by(|left, right| game_display_cmp(left, right, &running_games));

    for game in &games {
        group_list_box.append(&build_game_card(
            game,
            overlay,
            window,
            ui,
            &running_games,
            &ui.group_running_duration_labels,
        ));
    }

    finish_list_swap(
        &ui.group_list_stack,
        ui.group_list_showing_primary.as_ref(),
        visible_page,
    );
    ui.group_content_stack.set_visible_child_name("list");
}

fn build_group_card(
    group: &GameGroup,
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
    ui: &LibraryUi,
    running_games: &RunningGameMap,
    running_duration_labels: &Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
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

    let icon = build_library_icon(group_icon_file(&group.id), "folder", gtk4::Align::Start);

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
    let running_count = group
        .games
        .iter()
        .filter(|game| game_is_running(running_games, &game.id))
        .count();
    let meta_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
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
    meta_row.append(&count_label);
    if running_count > 0 {
        meta_row.append(
            &gtk4::Label::builder()
                .label(format!("{} running", running_count))
                .xalign(0.0)
                .css_classes(["caption", "accent"])
                .build(),
        );
    }
    let group_running_elapsed = group_running_elapsed_seconds(group, running_games);
    let status_label = gtk4::Label::builder()
        .label(if let Some(elapsed_seconds) = group_running_elapsed {
            format!("Running for {}", format_duration_brief(elapsed_seconds))
        } else {
            format_last_played(group_last_played(group))
        })
        .xalign(0.0)
        .css_classes(if group_running_elapsed.is_some() {
            ["caption", "accent"]
        } else {
            ["caption", "dim-label"]
        })
        .build();

    info_column.append(&title_label);
    info_column.append(&meta_row);
    info_column.append(&status_label);
    if group_running_elapsed.is_some() {
        running_duration_labels
            .borrow_mut()
            .insert(group.id.clone(), status_label.clone());
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
    running_games: &RunningGameMap,
    running_duration_labels: &Rc<RefCell<std::collections::HashMap<String, gtk4::Label>>>,
) -> gtk4::Frame {
    let game_running = game_is_running(running_games, &game.id);
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
        game_icon_file(&game.id),
        "application-x-executable-symbolic",
        gtk4::Align::Start,
    );

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
    info_column.append(&{
        let status_label = gtk4::Label::builder()
            .label(if game_running {
                format!(
                    "Running for {}",
                    format_duration_brief(
                        running_game_elapsed_seconds(running_games, &game.id).unwrap_or(0)
                    )
                )
            } else {
                format_last_played(game.last_played_epoch_seconds)
            })
            .xalign(0.0)
            .css_classes(if game_running {
                ["caption", "accent"]
            } else {
                ["caption", "dim-label"]
            })
            .build();
        if game_running {
            running_duration_labels
                .borrow_mut()
                .insert(game.id.clone(), status_label.clone());
        }
        status_label
    });

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
    css.load_from_string(&format!(
        "image.edit-icon {{ min-width: 0px; min-height: 0px; margin: 0px; padding: 0px; opacity: 0; }} \
         .library-icon-frame {{ min-width: {0}px; min-height: {0}px; border-radius: {1}px; padding: 0px; margin: 0px; background: transparent; }} \
         .library-icon-media {{ border-radius: {1}px; }}",
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
        .transition_duration(180)
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
        .transition_duration(180)
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

    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeftRight)
        .transition_duration(260)
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
        if current_version != running_state_version.get() {
            running_state_version.set(current_version);
            refresh_library_view(&ui_refresh, &overlay_refresh, &window_refresh);
        } else if current_version != 0 {
            update_running_duration_labels(&ui_refresh);
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

    let open_logs_on_start = crate::cli::take_open_logs_on_start();
    if open_logs_on_start {
        show_log_window(&window, None);
    }
}
