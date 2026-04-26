use std::fs;
use std::path::PathBuf;

use crate::config::normalize_game_id_from_executable;
use crate::icons::game_icon_file;
use crate::models::{Game, GameGroup};

pub fn desktop_entry_exists(leyen_id: &str) -> bool {
    !desktop_entry_paths_for_leyen_id(leyen_id).is_empty()
}

pub fn create_game_desktop_entry(
    game: &Game,
    group: Option<&GameGroup>,
) -> Result<PathBuf, String> {
    let path = desired_desktop_entry_path(game, group);
    ensure_applications_dir()?;
    for existing in desktop_entry_paths_for_leyen_id(&game.leyen_id) {
        if existing != path && existing.exists() {
            fs::remove_file(&existing).map_err(|err| {
                format!(
                    "Failed to remove desktop entry '{}': {}",
                    existing.display(),
                    err
                )
            })?;
        }
    }
    let icon = desktop_icon(game);
    fs::write(&path, render_game_desktop_entry(game, group, &icon)).map_err(|err| {
        format!(
            "Failed to write desktop entry '{}': {}",
            path.display(),
            err
        )
    })?;
    Ok(path)
}

pub fn update_game_desktop_entry_if_present(
    game: &Game,
    group: Option<&GameGroup>,
) -> Result<bool, String> {
    if !desktop_entry_exists(&game.leyen_id) {
        return Ok(false);
    }

    create_game_desktop_entry(game, group)?;
    Ok(true)
}

pub fn update_group_desktop_entries_if_present(group: &GameGroup) -> Result<usize, String> {
    let mut updated = 0usize;
    for game in &group.games {
        if update_game_desktop_entry_if_present(game, Some(group))? {
            updated += 1;
        }
    }
    Ok(updated)
}

pub fn remove_game_desktop_entry(leyen_id: &str) -> Result<bool, String> {
    let paths = desktop_entry_paths_for_leyen_id(leyen_id);
    let had_desktop_file = !paths.is_empty();

    for path in paths {
        fs::remove_file(&path).map_err(|err| {
            format!(
                "Failed to remove desktop entry '{}': {}",
                path.display(),
                err
            )
        })?;
    }
    Ok(had_desktop_file)
}

fn render_game_desktop_entry(game: &Game, group: Option<&GameGroup>, icon: &str) -> String {
    let display_name = display_name(game, group);
    let comment_name = sanitize_desktop_value(&display_name);
    let startup_wm_class = startup_wm_class(game);

    format!(
        "[Desktop Entry]\nVersion=1.0\nType=Application\nName={display_name}\nComment=Launch {comment_name} with Leyen\nExec=leyen run {leyen_id}\nIcon={icon}\nTerminal=false\nCategories=Game;\nStartupNotify=true\nStartupWMClass={startup_wm_class}\n",
        leyen_id = game.leyen_id
    )
}

fn display_name(game: &Game, group: Option<&GameGroup>) -> String {
    let game_title = sanitize_desktop_value(&game.title);
    match group {
        Some(group) => format!("{}: {}", sanitize_desktop_value(&group.title), game_title),
        None => game_title,
    }
}

fn startup_wm_class(game: &Game) -> String {
    let normalized = normalize_game_id_from_executable(&game.exe_path);
    if normalized.trim().is_empty() {
        game.game_id.trim().to_ascii_lowercase()
    } else {
        normalized
    }
}

fn desktop_icon(game: &Game) -> String {
    game_icon_file(&game.id)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| crate::APP_ID.to_string())
}

fn desired_desktop_entry_path(game: &Game, group: Option<&GameGroup>) -> PathBuf {
    applications_dir_path().join(desktop_entry_file_name(game, group))
}

fn desktop_entry_file_name(game: &Game, group: Option<&GameGroup>) -> String {
    format!(
        "{}.desktop",
        sanitize_desktop_file_name(&display_name(game, group))
    )
}

fn ensure_applications_dir() -> Result<PathBuf, String> {
    let path = applications_dir_path();
    fs::create_dir_all(&path).map_err(|err| {
        format!(
            "Failed to create applications directory '{}': {}",
            path.display(),
            err
        )
    })?;
    Ok(path)
}

fn applications_dir_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/applications")
}

fn desktop_entry_paths_for_leyen_id(leyen_id: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(applications_dir_path()) else {
        return Vec::new();
    };

    let exec_line = format!("Exec=leyen run {}", leyen_id.trim());
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
        })
        .filter(|path| {
            fs::read_to_string(path)
                .ok()
                .is_some_and(|content| content.lines().any(|line| line.trim() == exec_line))
        })
        .collect()
}

fn sanitize_desktop_value(value: &str) -> String {
    let sanitized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "Leyen".to_string()
    } else {
        sanitized
    }
}

fn sanitize_desktop_file_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '/' | '\0' => '-',
            '\n' | '\r' | '\t' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect::<String>();
    sanitize_desktop_value(&sanitized)
}

#[cfg(test)]
mod tests {
    use super::{
        desktop_entry_file_name, desktop_icon, render_game_desktop_entry, startup_wm_class,
    };
    use crate::models::{Game, GameGroup, GroupLaunchDefaults};

    fn sample_game() -> Game {
        Game {
            id: "game-1".to_string(),
            title: "Nier Replicant".to_string(),
            exe_path: "/games/NieR.exe".to_string(),
            prefix_path: String::new(),
            proton: "Default".to_string(),
            launch_args: String::new(),
            force_mangohud: false,
            force_gamemode: false,
            custom_icon: false,
            game_wayland: false,
            game_wow64: false,
            game_ntsync: false,
            leyen_id: "ly-1234".to_string(),
            game_id: "nier.exe".to_string(),
            playtime_seconds: 0,
            last_played_epoch_seconds: 0,
            last_run_duration_seconds: 0,
            last_run_status: String::new(),
        }
    }

    #[test]
    fn startup_wm_class_uses_lowercased_executable_name() {
        assert_eq!(startup_wm_class(&sample_game()), "nier.exe");
    }

    #[test]
    fn desktop_entry_uses_cli_run_command() {
        let rendered = render_game_desktop_entry(&sample_game(), None, crate::APP_ID);
        assert!(rendered.contains("Exec=leyen run ly-1234"));
        assert!(rendered.contains("Name=Nier Replicant"));
        assert!(rendered.contains("StartupWMClass=nier.exe"));
    }

    #[test]
    fn grouped_game_name_includes_group_title() {
        let rendered = render_game_desktop_entry(
            &sample_game(),
            Some(&GameGroup {
                id: "group-1".to_string(),
                title: "Favorites".to_string(),
                defaults: GroupLaunchDefaults::default(),
                games: Vec::new(),
            }),
            crate::APP_ID,
        );
        assert!(rendered.contains("Name=Favorites: Nier Replicant"));
    }

    #[test]
    fn desktop_file_name_uses_game_or_group_display_name() {
        assert_eq!(
            desktop_entry_file_name(&sample_game(), None),
            "Nier Replicant.desktop"
        );
        assert_eq!(
            desktop_entry_file_name(
                &sample_game(),
                Some(&GameGroup {
                    id: "group-1".to_string(),
                    title: "Favorites".to_string(),
                    defaults: GroupLaunchDefaults::default(),
                    games: Vec::new(),
                }),
            ),
            "Favorites: Nier Replicant.desktop"
        );
    }

    #[test]
    fn desktop_icon_falls_back_to_app_id_without_game_icon() {
        assert_eq!(desktop_icon(&sample_game()), crate::APP_ID);
    }
}
