//! Managed-icon path/name helpers (pure). Icon *extraction* (PE/ICO/PNG decode)
//! lives in the GUI client, which is the only thing that writes these files; the
//! daemon and CLI only need to compute or check paths.

use std::path::PathBuf;

pub fn game_icon_path(game_id: &str) -> PathBuf {
    managed_icons_dir_path().join(format!("{}.png", game_icon_name(game_id)))
}

pub fn group_icon_path(group_id: &str) -> PathBuf {
    managed_icons_dir_path().join(format!("{}.png", group_icon_name(group_id)))
}

pub fn game_icon_name(game_id: &str) -> String {
    format!("{}.game-{}", crate::APP_ID, game_id)
}

pub fn group_icon_name(group_id: &str) -> String {
    format!("{}.group-{}", crate::APP_ID, group_id)
}

pub fn game_icon_file(game_id: &str) -> Option<PathBuf> {
    let path = game_icon_path(game_id);
    path.is_file().then_some(path)
}

pub fn group_icon_file(group_id: &str) -> Option<PathBuf> {
    let path = group_icon_path(group_id);
    path.is_file().then_some(path)
}

pub fn managed_icons_dir_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/icons/hicolor/256x256/apps")
}

#[cfg(test)]
mod tests {
    use super::{game_icon_path, group_icon_path};

    #[test]
    fn managed_icon_paths_use_hicolor_app_directory() {
        let game_path = game_icon_path("game-1");
        let group_path = group_icon_path("group-1");
        let game_rendered = game_path.to_string_lossy();
        let group_rendered = group_path.to_string_lossy();

        assert!(game_rendered.contains(".local/share/icons/hicolor/256x256/apps"));
        assert!(group_rendered.contains(".local/share/icons/hicolor/256x256/apps"));
        assert!(game_rendered.ends_with("com.github.sachesi.leyen.game-game-1.png"));
        assert!(group_rendered.ends_with("com.github.sachesi.leyen.group-group-1.png"));
    }
}
