//! The dialogs: adding and editing games and groups, the preferences, and the
//! dependency manager they open, with what they share.

mod dependencies;
mod dependency_row;
mod game;
mod group;
mod preferences;
mod prefix_tools_group;

pub use dependencies::DependenciesPage;
pub use game::GameDialog;
pub use group::GroupDialog;
pub use preferences::PreferencesDialog;
pub use prefix_tools_group::{PrefixToolsGroup, ToolTarget};

use std::path::{Path, PathBuf};

use gtk4::prelude::*;
use leyen_model::i18n::gettext;
use leyen_model::models::GlobalSettings;
use libadwaita as adw;

use crate::daemon::gio_blocking;
use crate::icons::{
    clear_game_icon, clear_group_icon, extract_game_icon, save_custom_game_icon,
    save_custom_group_icon,
};

/// The Proton versions a combo row offers: the values settings store, and the
/// names the row shows for them.
pub struct ProtonChoices {
    values: Vec<String>,
    pub model: gtk4::StringList,
}

impl ProtonChoices {
    pub fn new(settings: &GlobalSettings) -> Self {
        let values = settings.available_proton_versions.clone();
        let names: Vec<String> = values
            .iter()
            .map(|value| {
                if value == "Default" {
                    gettext("Default")
                } else {
                    Path::new(value)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| value.clone())
                }
            })
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        Self {
            model: gtk4::StringList::new(&names),
            values,
        }
    }

    /// The position of `value`, or of "Default" when it is not offered.
    pub fn position(&self, value: &str) -> u32 {
        self.values
            .iter()
            .position(|candidate| candidate == value)
            .unwrap_or(0) as u32
    }

    pub fn value(&self, position: u32) -> String {
        self.values
            .get(position as usize)
            .cloned()
            .unwrap_or_else(|| "Default".to_string())
    }
}

/// Whether a chosen Proton version is still installed. "Default" always is.
pub async fn proton_exists(proton: &str) -> bool {
    if proton == "Default" {
        return true;
    }
    let path = PathBuf::from(proton);
    gio_blocking(move || path.exists()).await.unwrap_or(false)
}

pub fn windows_programs_filter() -> gtk4::FileFilter {
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some(&gettext("Windows programs")));
    for suffix in ["exe", "msi", "bat", "cmd", "com"] {
        filter.add_suffix(suffix);
    }
    filter
}

fn image_filter() -> gtk4::FileFilter {
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some(&gettext("Supported images")));
    for suffix in ["png", "jpg", "jpeg", "ico"] {
        filter.add_suffix(suffix);
    }
    filter
}

fn parent_window(widget: &impl IsA<gtk4::Widget>) -> Option<gtk4::Window> {
    widget.root().and_downcast::<gtk4::Window>()
}

/// Asks for a file and puts its path in `row`.
async fn choose_file_into(
    row: &(impl IsA<gtk4::Editable> + IsA<gtk4::Widget>),
    title: &str,
    filter: gtk4::FileFilter,
) {
    let dialog = gtk4::FileDialog::builder()
        .title(title)
        .default_filter(&filter)
        .build();
    if let Ok(file) = dialog.open_future(parent_window(row).as_ref()).await
        && let Some(path) = file.path()
    {
        row.set_text(&path.to_string_lossy());
    }
}

/// Asks for a folder and puts its path in `row`.
async fn choose_folder_into(row: &(impl IsA<gtk4::Editable> + IsA<gtk4::Widget>), title: &str) {
    let dialog = gtk4::FileDialog::builder().title(title).build();
    if let Ok(file) = dialog
        .select_folder_future(parent_window(row).as_ref())
        .await
        && let Some(path) = file.path()
    {
        row.set_text(&path.to_string_lossy());
    }
}

/// Writes the game's managed icon, from the custom file or from the executable.
/// `Ok(Some(_))` is a notice for the user: the executable had no icon.
async fn apply_game_icon(
    game_id: String,
    exe_path: String,
    custom_icon: Option<String>,
) -> Result<Option<String>, String> {
    gio_blocking(move || match custom_icon {
        Some(icon_file) if icon_file.trim().is_empty() => {
            Err(gettext("Custom icon file is required"))
        }
        Some(icon_file) => save_custom_game_icon(&game_id, &icon_file).map(|()| None),
        None => match extract_game_icon(&game_id, &exe_path) {
            Ok(()) => Ok(None),
            Err(_) => {
                clear_game_icon(&game_id);
                Ok(Some(gettext(
                    "No icon could be extracted from the executable; using the default symbol.",
                )))
            }
        },
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

/// Writes the group's managed icon from the custom file, or removes it.
async fn apply_group_icon(group_id: String, custom_icon: Option<String>) -> Result<(), String> {
    gio_blocking(move || match custom_icon {
        Some(icon_file) if icon_file.trim().is_empty() => {
            Err(gettext("Custom icon file is required"))
        }
        Some(icon_file) => save_custom_group_icon(&group_id, &icon_file),
        None => {
            clear_group_icon(&group_id);
            Ok(())
        }
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

/// Keeps a prefix row in step with its "Custom Prefix" switch and the title:
/// switching it off remembers what was typed, switching it on brings that back or
/// suggests a folder named after the title, and a suggestion follows the title
/// until it is edited.
#[derive(Default)]
pub struct PrefixSuggestion {
    default_prefix: String,
    /// What the row held when the switch was last turned off.
    stored: String,
    /// The last folder suggested for the title.
    suggested: String,
}

impl PrefixSuggestion {
    fn new(default_prefix: &str, initial: &str) -> Self {
        Self {
            default_prefix: default_prefix.to_string(),
            stored: initial.to_string(),
            suggested: initial.to_string(),
        }
    }

    fn toggled(&mut self, enabled: bool, prefix_row: &adw::EntryRow, title: &str) {
        if enabled {
            let text = if self.stored.trim().is_empty() {
                leyen_model::library::suggest_prefix_path(&self.default_prefix, title)
            } else {
                self.stored.clone()
            };
            prefix_row.set_text(&text);
        } else {
            self.stored = prefix_row.text().to_string();
            prefix_row.set_text("");
        }
    }

    fn title_changed(&mut self, prefix_row: &adw::EntryRow, title: &str) {
        let suggestion = leyen_model::library::suggest_prefix_path(&self.default_prefix, title);
        let current = prefix_row.text();
        if current.trim().is_empty() || current == self.suggested || current == self.default_prefix
        {
            prefix_row.set_text(&suggestion);
        }
        self.suggested = suggestion;
    }
}
