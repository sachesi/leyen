use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::daemon::gio_blocking;
use leyen_model::i18n::gettext;
use leyen_model::icons::game_icon_file;
use leyen_model::library::{effective_game_id, is_leyen_id};
use leyen_model::models::{Game, GameGroup};

pub fn desktop_entry_exists(leyen_id: &str) -> bool {
    !desktop_entry_paths_for_leyen_id(leyen_id).is_empty()
}

pub async fn create_game_desktop_entry(
    game: Game,
    group: Option<GameGroup>,
) -> Result<PathBuf, String> {
    gio_blocking(move || {
        let existing = desktop_entry_paths_for_leyen_id(&game.leyen_id);
        write_game_desktop_entry(&game, group.as_ref(), &existing)
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

pub async fn update_game_desktop_entry_if_present(
    game: Game,
    group: Option<GameGroup>,
) -> Result<bool, String> {
    gio_blocking(move || {
        let existing = desktop_entry_paths_for_leyen_id(&game.leyen_id);
        if existing.is_empty() {
            return Ok(false);
        }
        write_game_desktop_entry(&game, group.as_ref(), &existing).map(|_| true)
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

/// Rewrites the menu entries of the group's games that have one, reading the
/// applications folder once for all of them.
pub async fn update_group_desktop_entries_if_present(group: GameGroup) -> Result<usize, String> {
    gio_blocking(move || {
        let mut owned = owned_desktop_entries();
        let mut updated = 0usize;
        for game in &group.games {
            if let Some(existing) = owned.remove(game.leyen_id.trim()) {
                write_game_desktop_entry(game, Some(&group), &existing)?;
                updated += 1;
            }
        }
        Ok(updated)
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

/// Writes the game's menu entry and removes any other file that was its entry.
fn write_game_desktop_entry(
    game: &Game,
    group: Option<&GameGroup>,
    existing: &[PathBuf],
) -> Result<PathBuf, String> {
    // The id goes into the Exec line as it is; one edited by hand in the library
    // could otherwise add lines of its own to the entry.
    if !is_leyen_id(&game.leyen_id) {
        return Err(format!("unexpected game ID {:?}", game.leyen_id));
    }
    let path = desired_desktop_entry_path(game, group);
    ensure_applications_dir()?;
    // `existing` may come from one read for a whole group, before earlier games
    // were written: remove a file only while it is still this game's entry.
    for old in existing {
        if *old != path
            && fs::read_to_string(old).is_ok_and(|c| content_owns_leyen_id(&c, &game.leyen_id))
        {
            fs::remove_file(old).map_err(|err| format!("{}: {err}", old.display()))?;
        }
    }
    let icon = desktop_icon(game);
    leyen_model::paths::atomic_write(&path, &render_game_desktop_entry(game, group, &icon))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

pub async fn remove_game_desktop_entry(leyen_id: String) -> Result<bool, String> {
    gio_blocking(move || {
        let paths = desktop_entry_paths_for_leyen_id(&leyen_id);
        let had_desktop_file = !paths.is_empty();

        for path in paths {
            fs::remove_file(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        }
        Ok(had_desktop_file)
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

/// Removes the menu entries of all these games, reading the applications folder
/// once. Keeps going past a file that cannot be removed and reports the last one.
pub async fn remove_game_desktop_entries(leyen_ids: Vec<String>) -> Result<(), String> {
    gio_blocking(move || {
        let mut owned = owned_desktop_entries();
        let mut result = Ok(());
        for path in leyen_ids
            .iter()
            .filter_map(|id| owned.remove(id.trim()))
            .flatten()
        {
            if let Err(err) = fs::remove_file(&path) {
                result = Err(format!("{}: {err}", path.display()));
            }
        }
        result
    })
    .await
    .unwrap_or_else(|| Err(gettext("Internal error: background task failed")))
}

fn render_game_desktop_entry(game: &Game, group: Option<&GameGroup>, icon: &str) -> String {
    // A backslash starts an escape in a desktop entry value.
    let display_name = display_name(game, group).replace('\\', "\\\\");
    let comment_name = &display_name;
    let startup_wm_class = startup_wm_class(game);
    let leyen_id = shlex::try_quote(&game.leyen_id).unwrap_or(Cow::Borrowed(&game.leyen_id));

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
    fs::create_dir_all(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

fn applications_dir_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.data_dir().join("applications"))
        .unwrap_or_else(|| PathBuf::from(".local/share/applications"))
}

fn desktop_entry_paths_for_leyen_id(leyen_id: &str) -> Vec<PathBuf> {
    owned_desktop_entries()
        .remove(leyen_id.trim())
        .unwrap_or_default()
}

/// Every menu entry Leyen wrote, whichever game it launches.
pub fn owned_desktop_entry_paths() -> Vec<PathBuf> {
    owned_desktop_entries().into_values().flatten().collect()
}

/// The menu entries Leyen wrote, by the Leyen ID their `Exec=leyen run` line
/// launches, from one pass over the applications folder.
fn owned_desktop_entries() -> HashMap<String, Vec<PathBuf>> {
    owned_desktop_entries_in(&applications_dir_path())
}

fn owned_desktop_entries_in(dir: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut owned: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return owned;
    };
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        let is_desktop = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"));
        let Some(content) = is_desktop.then(|| fs::read_to_string(&path).ok()).flatten() else {
            continue;
        };
        let ids: HashSet<&str> = content
            .lines()
            .filter_map(|line| line.trim().strip_prefix("Exec=leyen run "))
            .collect();
        for id in ids {
            owned.entry(id.to_string()).or_default().push(path.clone());
        }
    }
    owned
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
        desktop_entry_file_name, desktop_icon, disambiguate_file_name, owned_desktop_entries_in,
        render_game_desktop_entry, startup_wm_class, write_game_desktop_entry,
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
            wayland: false,
            wow64: false,
            ntsync: false,
            hdr: false,
            proton_log: false,
            sandbox_network: Default::default(),
            sandbox_folders: Vec::new(),
            game_dir: "/games/NieR".to_string(),
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
    fn no_entry_is_written_for_a_game_id_leyen_did_not_give() {
        let mut game = sample_game();
        game.leyen_id = "ly-1234\nExec=true".to_string();
        assert!(write_game_desktop_entry(&game, None, &[]).is_err());
    }

    #[test]
    fn a_backslash_in_a_title_is_escaped() {
        let mut game = sample_game();
        game.title = "C:\\Games\\Nier".to_string();
        let rendered = render_game_desktop_entry(&game, None, leyen_model::APP_ID);
        assert!(rendered.contains("Name=C:\\\\Games\\\\Nier\n"));
        assert!(rendered.contains("Comment=Launch C:\\\\Games\\\\Nier with Leyen\n"));
    }

    #[test]
    fn menu_entries_are_found_by_the_game_they_launch() {
        let dir = std::env::temp_dir().join(format!("leyen-menu-entries-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, content: &str| std::fs::write(dir.join(name), content).unwrap();
        write("A.desktop", "[Desktop Entry]\nExec=leyen run ly-1\n");
        write("B.desktop", "[Desktop Entry]\n  Exec=leyen run ly-2  \n");
        write("Other.desktop", "[Desktop Entry]\nExec=other-app\n");
        write("notes.txt", "Exec=leyen run ly-3\n");

        let owned = owned_desktop_entries_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(owned.len(), 2);
        assert_eq!(owned["ly-1"], [dir.join("A.desktop")]);
        assert_eq!(owned["ly-2"], [dir.join("B.desktop")]);
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
