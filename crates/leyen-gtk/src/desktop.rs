use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use crate::daemon::gio_blocking;
use leyen_model::icons::game_icon_file;
use leyen_model::library::effective_game_id;
use leyen_model::models::{Game, GameGroup};

pub fn desktop_entry_exists(leyen_id: &str) -> bool {
    !desktop_entry_paths_for_leyen_id(leyen_id).is_empty()
}

pub async fn create_game_desktop_entry(
    game: Game,
    group: Option<GameGroup>,
) -> Result<PathBuf, String> {
    gio_blocking(move || {
        let path = desired_desktop_entry_path(&game, group.as_ref());
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
        let icon = desktop_icon(&game);
        fs::write(
            &path,
            render_game_desktop_entry(&game, group.as_ref(), &icon),
        )
        .map_err(|err| {
            format!(
                "Failed to write desktop entry '{}': {}",
                path.display(),
                err
            )
        })?;
        Ok(path)
    })
    .await
    .unwrap_or_else(|| Err("Internal error: background task failed".to_string()))
}

pub async fn update_game_desktop_entry_if_present(
    game: Game,
    group: Option<GameGroup>,
) -> Result<bool, String> {
    let leyen_id = game.leyen_id.clone();
    if !gio_blocking(move || desktop_entry_exists(&leyen_id))
        .await
        .unwrap_or(false)
    {
        return Ok(false);
    }

    create_game_desktop_entry(game, group).await?;
    Ok(true)
}

pub async fn update_group_desktop_entries_if_present(group: GameGroup) -> Result<usize, String> {
    let mut updated = 0usize;
    for game in group.games.clone() {
        if update_game_desktop_entry_if_present(game, Some(group.clone())).await? {
            updated += 1;
        }
    }
    Ok(updated)
}

pub async fn remove_game_desktop_entry(leyen_id: String) -> Result<bool, String> {
    gio_blocking(move || {
        let paths = desktop_entry_paths_for_leyen_id(&leyen_id);
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
    })
    .await
    .unwrap_or_else(|| Err("Internal error: background task failed".to_string()))
}

fn render_game_desktop_entry(game: &Game, group: Option<&GameGroup>, icon: &str) -> String {
    let display_name = display_name(game, group);
    let comment_name = sanitize_desktop_value(&display_name);
    let startup_wm_class = startup_wm_class(game);
    let leyen_id = shlex::try_quote(&game.leyen_id)
        .unwrap_or(Cow::Borrowed(&game.leyen_id));

    format!(
        "[Desktop Entry]\nVersion=1.0\nType=Application\nName={display_name}\nComment=Launch {comment_name} with Leyen\nExec=leyen run {leyen_id}\nIcon={icon}\nTerminal=false\nCategories=Game;\nStartupNotify=true\nStartupWMClass={startup_wm_class}\n"
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
    format!(
        "steam_app_{}",
        effective_game_id(game).trim_start_matches("umu-")
    )
}

fn desktop_icon(game: &Game) -> String {
    game_icon_file(&game.id)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| leyen_model::APP_ID.to_string())
}

fn desired_desktop_entry_path(game: &Game, group: Option<&GameGroup>) -> PathBuf {
    let dir = applications_dir_path();
    let plain_name = desktop_entry_file_name(game, group);
    let existing_content = fs::read_to_string(dir.join(&plain_name)).ok();
    let file_name =
        disambiguate_file_name(&plain_name, &game.leyen_id, existing_content.as_deref());
    dir.join(file_name)
}

fn disambiguate_file_name(
    plain_name: &str,
    leyen_id: &str,
    existing_content: Option<&str>,
) -> String {
    match existing_content {
        Some(content) if !content_owns_leyen_id(content, leyen_id) => {
            let stem = plain_name.strip_suffix(".desktop").unwrap_or(plain_name);
            let suffix: String = leyen_id.chars().take(8).collect();
            format!("{stem}-{suffix}.desktop")
        }
        _ => plain_name.to_string(),
    }
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
    directories::BaseDirs::new()
        .map(|base| base.data_dir().join("applications"))
        .unwrap_or_else(|| PathBuf::from(".local/share/applications"))
}

fn desktop_entry_paths_for_leyen_id(leyen_id: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(applications_dir_path()) else {
        return Vec::new();
    };

    let mut result = Vec::new();

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let is_desktop = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"));
        if is_desktop
            && let Ok(content) = fs::read_to_string(&path)
            && content_owns_leyen_id(&content, leyen_id)
        {
            result.push(path);
        }
    }
    result
}

fn content_owns_leyen_id(content: &str, leyen_id: &str) -> bool {
    let exec_line = format!("Exec=leyen run {}", leyen_id.trim());
    content.lines().any(|line| line.trim() == exec_line)
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
        desktop_entry_file_name, desktop_icon, disambiguate_file_name, render_game_desktop_entry,
        startup_wm_class,
    };
    use leyen_model::models::{Game, GameGroup, GroupLaunchDefaults};

    fn sample_game() -> Game {
        Game {
            id: "game-1".to_string(),
            title: "Nier Replicant".to_string(),
            exe_path: "/games/NieR.exe".to_string(),
            prefix_path: String::new(),
            proton: "Default".to_string(),
            launch_args: String::new(),
            mangohud: false,
            gamemode: false,
            wayland: false,
            wow64: false,
            ntsync: false,
            hdr: false,
            proton_log: false,
            custom_icon: false,
            leyen_id: "ly-1234".to_string(),
            game_id: "nier.exe".to_string(),
            playtime_seconds: 0,
            last_played_epoch_seconds: 0,
            last_run_duration_seconds: 0,
            last_run_status: String::new(),
        }
    }

    #[test]
    fn startup_wm_class_uses_umu_steam_app_id() {
        assert_eq!(startup_wm_class(&sample_game()), "steam_app_ly1234");
    }

    #[test]
    fn desktop_entry_uses_cli_run_command() {
        let rendered = render_game_desktop_entry(&sample_game(), None, leyen_model::APP_ID);
        assert!(rendered.contains("Exec=leyen run ly-1234"));
        assert!(rendered.contains("Name=Nier Replicant"));
        assert!(rendered.contains("StartupWMClass=steam_app_ly1234"));
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
            leyen_model::APP_ID,
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
        assert_eq!(desktop_icon(&sample_game()), leyen_model::APP_ID);
    }

    #[test]
    fn disambiguate_keeps_plain_name_when_no_collision() {
        assert_eq!(
            disambiguate_file_name("Nier Replicant.desktop", "ly-1234", None),
            "Nier Replicant.desktop"
        );
    }

    #[test]
    fn disambiguate_keeps_plain_name_when_collision_is_own_entry() {
        let own_content = "[Desktop Entry]\nExec=leyen run ly-1234\n";
        assert_eq!(
            disambiguate_file_name("Nier Replicant.desktop", "ly-1234", Some(own_content)),
            "Nier Replicant.desktop"
        );
    }

    #[test]
    fn disambiguate_suffixes_name_when_collision_is_foreign() {
        let foreign_content = "[Desktop Entry]\nExec=some-other-app\n";
        assert_eq!(
            disambiguate_file_name("Nier Replicant.desktop", "ly-1234", Some(foreign_content)),
            "Nier Replicant-ly-1234.desktop"
        );
    }
}
