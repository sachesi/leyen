use crate::models::LibraryItem;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub const LIBRARY_ICON_SIZE: i32 = 48;
pub const LIBRARY_ICON_CORNER_RADIUS: i32 = 7;
pub const MAIN_WINDOW_DEFAULT_WIDTH: i32 = 540;
pub const MAIN_WINDOW_DEFAULT_HEIGHT: i32 = 640;
pub const SECONDARY_WINDOW_DEFAULT_WIDTH: i32 = MAIN_WINDOW_DEFAULT_WIDTH - 20;
pub const SECONDARY_WINDOW_DEFAULT_HEIGHT: i32 = MAIN_WINDOW_DEFAULT_HEIGHT - 20;

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
    pub _search_bar: gtk4::SearchBar,
    pub search_entry: gtk4::SearchEntry,
    pub library_state: Rc<RefCell<Vec<LibraryItem>>>,
    pub current_group_id: Rc<RefCell<Option<String>>>,
}
