//! Pure, read-only library helpers shared by clients and the daemon.
//!
//! Parsing/serialization and in-memory mutation of the library live here so the
//! thin clients can read `games.toml` and compute edits without pulling in the
//! engine. The daemon is the only process that *persists* the result (see
//! `leyen-core`'s config writer).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::models::{GAMES_CONFIG_VERSION, Game, GameGroup, GamesConfig, GroupLaunchDefaults, LibraryItem};
use crate::paths::get_config_path;

const LEYEN_ID_PREFIX: &str = "ly-";
const LEYEN_ID_DIGITS: usize = 4;

/// Reads and parses `games.toml`. Returns an empty library if the file is
/// absent; an error string if it exists but cannot be read or parsed (callers
/// must never treat that as "empty" — overwriting would erase the library).
pub fn read_library_from_disk() -> Result<Vec<LibraryItem>, String> {
    let path = get_config_path();
    match fs::read_to_string(&path) {
        Ok(data) => toml::from_str::<GamesConfig>(&data)
            .map_err(|e| format!("Failed to parse games config: {e}"))
            .and_then(|config| {
                if config.version > GAMES_CONFIG_VERSION {
                    return Err(format!(
                        "'{}' was written by a newer version of leyen (file version {}, this build supports {})",
                        path.display(),
                        config.version,
                        GAMES_CONFIG_VERSION
                    ));
                }
                Ok(config.items)
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Failed to read games config: {e}")),
    }
}

/// Serializes a library to TOML (the on-disk `games.toml` representation).
pub fn serialize_library(items: &[LibraryItem]) -> Result<String, String> {
    toml::to_string_pretty(&GamesConfig {
        version: GAMES_CONFIG_VERSION,
        items: items.to_vec(),
    })
    .map_err(|e| format!("Failed to serialize games config: {e}"))
}

pub fn flatten_games(items: &[LibraryItem]) -> Vec<Game> {
    items
        .iter()
        .flat_map(|item| match item {
            LibraryItem::Game(game) => std::slice::from_ref(game),
            LibraryItem::Group(group) => &group.games,
        })
        .cloned()
        .collect()
}

pub fn find_game_mut<'a>(items: &'a mut [LibraryItem], game_id: &str) -> Option<&'a mut Game> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some(game),
            LibraryItem::Group(group) => {
                for game in &mut group.games {
                    if game.id == game_id {
                        return Some(game);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_game_and_group<'a>(
    items: &'a [LibraryItem],
    game_id: &str,
) -> Option<(&'a Game, Option<&'a GameGroup>)> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some((game, None)),
            LibraryItem::Group(group) => {
                for game in &group.games {
                    if game.id == game_id {
                        return Some((game, Some(group)));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_game_by_leyen_id<'a>(
    items: &'a [LibraryItem],
    leyen_id: &str,
) -> Option<(&'a Game, Option<&'a GameGroup>)> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.leyen_id == leyen_id => return Some((game, None)),
            LibraryItem::Group(group) => {
                for game in &group.games {
                    if game.leyen_id == leyen_id {
                        return Some((game, Some(group)));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_group<'a>(items: &'a [LibraryItem], group_id: &str) -> Option<&'a GameGroup> {
    items.iter().find_map(|item| {
        if let LibraryItem::Group(group) = item
            && group.id == group_id
        {
            Some(group)
        } else {
            None
        }
    })
}

pub fn game_parent_group_id(items: &[LibraryItem], game_id: &str) -> Option<String> {
    items.iter().find_map(|item| {
        if let LibraryItem::Group(group) = item
            && group.games.iter().any(|g| g.id == game_id)
        {
            return Some(group.id.clone());
        }
        None
    })
}

pub fn insert_game(items: &mut Vec<LibraryItem>, group_id: Option<&str>, game: Game) -> bool {
    if let Some(gid) = group_id {
        for item in items {
            if let LibraryItem::Group(group) = item
                && group.id == gid
            {
                group.games.push(game);
                return true;
            }
        }
        false
    } else {
        items.push(LibraryItem::Game(game));
        true
    }
}

pub fn replace_game(items: &mut [LibraryItem], updated_game: &Game) -> bool {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == updated_game.id => {
                *game = updated_game.clone();
                return true;
            }
            LibraryItem::Group(group) => {
                for game in &mut group.games {
                    if game.id == updated_game.id {
                        *game = updated_game.clone();
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

pub fn replace_group(
    items: &mut [LibraryItem],
    group_id: &str,
    new_title: String,
    new_defaults: GroupLaunchDefaults,
) -> bool {
    for item in items {
        if let LibraryItem::Group(group) = item
            && group.id == group_id
        {
            group.title = new_title;
            group.defaults = new_defaults;
            return true;
        }
    }
    false
}

pub fn remove_game(items: &mut Vec<LibraryItem>, game_id: &str) -> Option<Game> {
    if let Some(pos) = items.iter().position(|item| {
        if let LibraryItem::Game(game) = item {
            game.id == game_id
        } else {
            false
        }
    }) && let LibraryItem::Game(game) = items.remove(pos)
    {
        return Some(game);
    }

    for item in items {
        if let LibraryItem::Group(group) = item
            && let Some(pos) = group.games.iter().position(|g| g.id == game_id)
        {
            return Some(group.games.remove(pos));
        }
    }
    None
}

pub fn remove_group(items: &mut Vec<LibraryItem>, group_id: &str) -> Option<GameGroup> {
    if let Some(pos) = items.iter().position(|item| {
        if let LibraryItem::Group(group) = item {
            group.id == group_id
        } else {
            false
        }
    }) && let LibraryItem::Group(group) = items.remove(pos)
    {
        return Some(group);
    }
    None
}

pub fn generate_unique_leyen_id(items: &[LibraryItem]) -> String {
    let existing_ids: HashSet<String> = flatten_games(items)
        .into_iter()
        .map(|g| g.leyen_id)
        .collect();

    for _ in 0..100 {
        let id = format!(
            "{}{:0width$}",
            LEYEN_ID_PREFIX,
            fastrand::u32(1..10u32.pow(LEYEN_ID_DIGITS as u32)),
            width = LEYEN_ID_DIGITS
        );
        if !existing_ids.contains(&id) {
            return id;
        }
    }

    // Sequential fallback if random attempts exhausted
    for n in 1..10u32.pow(LEYEN_ID_DIGITS as u32) {
        let id = format!("{}{:0width$}", LEYEN_ID_PREFIX, n, width = LEYEN_ID_DIGITS);
        if !existing_ids.contains(&id) {
            return id;
        }
    }

    // 4-digit pool fully exhausted — widen monotonically, guaranteeing uniqueness.
    let mut n = 10u32.pow(LEYEN_ID_DIGITS as u32);
    loop {
        let id = format!("{}{}", LEYEN_ID_PREFIX, n);
        if !existing_ids.contains(&id) {
            return id;
        }
        n += 1;
    }
}

#[cfg(test)]
pub(crate) fn is_valid_leyen_id(id: &str) -> bool {
    id.starts_with(LEYEN_ID_PREFIX)
        && id.len() == LEYEN_ID_PREFIX.len() + LEYEN_ID_DIGITS
        && id[LEYEN_ID_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

pub fn effective_game_id(game: &Game) -> String {
    umu_game_id(&game.leyen_id)
}

pub fn umu_game_id(leyen_id: &str) -> String {
    format!("umu-{}", leyen_id.replace('-', ""))
}

pub fn suggest_prefix_path(default_prefix: &str, title: &str) -> String {
    if default_prefix.is_empty() {
        return String::new();
    }
    let sanitized_title = title.to_lowercase().replace(' ', "-");
    let mut path = PathBuf::from(default_prefix);
    if let Some(parent) = path.parent() {
        path = parent.join(sanitized_title);
    } else {
        path = PathBuf::from(sanitized_title);
    }
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_leyen_id_matches_expected_shape() {
        assert!(is_valid_leyen_id("ly-2534"));
        assert!(!is_valid_leyen_id("ly-253"));
        assert!(!is_valid_leyen_id("ly-25a4"));
        assert!(!is_valid_leyen_id("game-2534"));
    }

    #[test]
    fn generate_unique_leyen_id_avoids_existing_ids() {
        let items = vec![
            LibraryItem::Game(Game {
                leyen_id: "ly-1234".to_string(),
                ..Game::default()
            }),
            LibraryItem::Group(GameGroup {
                id: "group-1".to_string(),
                title: "Group 1".to_string(),
                defaults: GroupLaunchDefaults::default(),
                games: vec![Game {
                    leyen_id: "ly-5678".to_string(),
                    ..Game::default()
                }],
            }),
        ];

        let generated = generate_unique_leyen_id(&items);
        assert!(is_valid_leyen_id(&generated));
        assert_ne!(generated, "ly-1234");
        assert_ne!(generated, "ly-5678");
    }

    #[test]
    fn effective_game_id_is_valid_unique_umu_id() {
        let game = Game {
            leyen_id: "ly-1234".to_string(),
            game_id: "shared.exe".to_string(),
            ..Game::default()
        };

        assert_eq!(effective_game_id(&game), "umu-ly1234");
    }
}
