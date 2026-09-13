//! The move from the `com.github.sachesi.leyen` application id to
//! `io.github.sachesi.leyen`. Managed icons carry the id in their file names and
//! the menu entries Leyen wrote name those icons, or the application icon, in
//! their `Icon=` line; both are brought over to the new id. Safe to run on every
//! start: once nothing carries the old id it lists the icons and reads the menu
//! entries Leyen wrote, and changes nothing.

use std::fs;

use leyen_model::APP_ID;
use leyen_model::icons::managed_icons_dir_path;

const LEGACY_APP_ID: &str = "com.github.sachesi.leyen";

pub fn migrate_legacy_app_id() {
    rename_managed_icons();
    for path in crate::desktop::owned_desktop_entry_paths() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(migrated) = migrate_desktop_entry(&content)
            && let Err(err) = leyen_model::paths::atomic_write(&path, &migrated)
        {
            log::warn!("Failed to update menu entry '{}': {err}", path.display());
        }
    }
}

fn rename_managed_icons() {
    let dir = managed_icons_dir_path();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let legacy_prefix = format!("{LEGACY_APP_ID}.");
    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let Some(rest) = file_name
            .to_str()
            .and_then(|name| name.strip_prefix(&legacy_prefix))
        else {
            continue;
        };
        if !(rest.starts_with("game-") || rest.starts_with("group-")) {
            continue;
        }
        let target = dir.join(format!("{APP_ID}.{rest}"));
        // An icon already under the new name was written by this version and wins.
        let result = if target.exists() {
            fs::remove_file(entry.path())
        } else {
            fs::rename(entry.path(), &target)
        };
        if let Err(err) = result {
            log::warn!("Failed to migrate icon '{}': {err}", entry.path().display());
        }
    }
}

/// The entry with its `Icon=` line pointing at the new id, or `None` when it
/// does not name the old one.
fn migrate_desktop_entry(content: &str) -> Option<String> {
    let mut changed = false;
    let mut migrated = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        match line.strip_prefix("Icon=") {
            Some(icon) if icon.contains(LEGACY_APP_ID) => {
                migrated.push_str("Icon=");
                migrated.push_str(&icon.replace(LEGACY_APP_ID, APP_ID));
                changed = true;
            }
            _ => migrated.push_str(line),
        }
    }
    changed.then_some(migrated)
}

#[cfg(test)]
mod tests {
    use super::migrate_desktop_entry;

    #[test]
    fn application_icon_moves_to_the_new_id() {
        let entry = "[Desktop Entry]\nExec=leyen run ly-1234\nIcon=com.github.sachesi.leyen\n";
        assert_eq!(
            migrate_desktop_entry(entry).as_deref(),
            Some("[Desktop Entry]\nExec=leyen run ly-1234\nIcon=io.github.sachesi.leyen\n")
        );
    }

    #[test]
    fn managed_icon_path_moves_to_the_new_file_name() {
        let entry = "Icon=/home/u/.local/share/icons/hicolor/256x256/apps/com.github.sachesi.leyen.game-1.png\nName=A";
        assert_eq!(
            migrate_desktop_entry(entry).as_deref(),
            Some(
                "Icon=/home/u/.local/share/icons/hicolor/256x256/apps/io.github.sachesi.leyen.game-1.png\nName=A"
            )
        );
    }

    #[test]
    fn entry_without_the_old_id_is_left_alone() {
        assert_eq!(
            migrate_desktop_entry(
                "Icon=io.github.sachesi.leyen\nComment=com.github.sachesi.leyen\n"
            ),
            None
        );
    }
}
