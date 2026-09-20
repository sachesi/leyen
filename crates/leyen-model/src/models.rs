use serde::{Deserialize, Serialize};

/// Whether a program reaches the network from inside its sandbox. A game takes
/// the group's answer, a group the global setting, and the global setting allows
/// it — a game that cannot reach the network is the exception, not the rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccess {
    #[default]
    Inherit,
    Allowed,
    Blocked,
}

/// Resolves the three levels into the answer a launch needs.
pub fn resolve_network_access(
    game: NetworkAccess,
    group: Option<NetworkAccess>,
    global: bool,
) -> bool {
    for level in [Some(game), group] {
        match level {
            Some(NetworkAccess::Allowed) => return true,
            Some(NetworkAccess::Blocked) => return false,
            _ => {}
        }
    }
    global
}

/// A folder the sandbox exposes besides the prefix and the game's own folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxFolder {
    pub path: String,
    pub writable: bool,
}

/// Every folder named at any of the three levels. A path named more than once
/// keeps what the narrowest level says about writing to it.
pub fn resolve_sandbox_folders(
    game: &[SandboxFolder],
    group: Option<&[SandboxFolder]>,
    global: &[SandboxFolder],
) -> Vec<SandboxFolder> {
    let mut resolved: Vec<SandboxFolder> = Vec::new();
    for level in [Some(game), group, Some(global)] {
        for folder in level.unwrap_or_default() {
            let path = folder.path.trim();
            if path.is_empty() || resolved.iter().any(|kept| kept.path == path) {
                continue;
            }
            resolved.push(SandboxFolder {
                path: path.to_string(),
                writable: folder.writable,
            });
        }
    }
    resolved
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub exe_path: String,
    pub prefix_path: String,
    pub proton: String,
    pub launch_args: String,
    pub mangohud: bool,
    pub wayland: bool,
    pub wow64: bool,
    pub ntsync: bool,
    pub hdr: bool,
    pub proton_log: bool,
    pub sandbox_network: NetworkAccess,
    /// Folders the sandbox exposes to this game on top of the ones its group and
    /// the preferences name.
    pub sandbox_folders: Vec<SandboxFolder>,
    /// The one folder of the user's the game sees. Set by hand, never derived
    /// from the executable: for some games it is the executable's folder, for
    /// others the install root above it. A game without one does not launch.
    pub game_dir: String,
    pub custom_icon: bool,
    pub leyen_id: String,
    pub game_id: String,
    pub playtime_seconds: u64,
    pub last_played_epoch_seconds: u64,
    pub last_run_duration_seconds: u64,
    pub last_run_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GroupLaunchDefaults {
    pub prefix_path: String,
    pub proton: String,
    pub sandbox_network: NetworkAccess,
    /// Folders the sandbox exposes to every game in the group.
    pub sandbox_folders: Vec<SandboxFolder>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GameGroup {
    pub id: String,
    pub title: String,
    pub defaults: GroupLaunchDefaults,
    pub games: Vec<Game>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LibraryItem {
    Game(Game),
    Group(GameGroup),
}

/// Current `settings.toml` schema version. Bump when a change to
/// `GlobalSettings` needs the reader/writer split below to distinguish
/// old files from new ones.
pub const GLOBAL_SETTINGS_VERSION: u32 = 1;

fn default_global_settings_version() -> u32 {
    GLOBAL_SETTINGS_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    #[serde(default = "default_global_settings_version")]
    pub version: u32,
    pub default_prefix_path: String,
    pub default_proton: String,
    pub global_mangohud: bool,
    pub global_wayland: bool,
    pub global_wow64: bool,
    pub global_ntsync: bool,
    pub global_hdr: bool,
    pub global_proton_log: bool,
    pub available_proton_versions: Vec<String>,
    pub log_errors: bool,
    pub log_warnings: bool,
    pub log_operations: bool,
    /// Whether a game reaches the network from inside its sandbox, for every
    /// game that does not answer for itself or through its group.
    #[serde(default = "default_true")]
    pub global_sandbox_network: bool,
    /// Folders the sandbox exposes to every game.
    pub global_sandbox_folders: Vec<SandboxFolder>,
}

/// Logging defaults to fully enabled: the Logs window is a primary debugging
/// surface, and a launcher that swallows its own errors is undebuggable.
impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            version: GLOBAL_SETTINGS_VERSION,
            default_prefix_path: String::new(),
            default_proton: String::new(),
            global_mangohud: false,
            global_wayland: false,
            global_wow64: false,
            global_ntsync: false,
            global_hdr: false,
            global_proton_log: false,
            available_proton_versions: Vec::new(),
            log_errors: true,
            log_warnings: true,
            log_operations: true,
            global_sandbox_network: true,
            global_sandbox_folders: Vec::new(),
        }
    }
}

/// Current `games.toml` schema version. Bump when a change to `GamesConfig`
/// needs the reader/writer split below to distinguish old files from new ones.
pub const GAMES_CONFIG_VERSION: u32 = 1;

fn default_games_config_version() -> u32 {
    GAMES_CONFIG_VERSION
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GamesConfig {
    #[serde(default = "default_games_config_version")]
    pub version: u32,
    pub items: Vec<LibraryItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_version_field_parses_as_one() {
        let config: GamesConfig = toml::from_str("items = []").unwrap();
        assert_eq!(config.version, 1);

        let settings: GlobalSettings = toml::from_str("").unwrap();
        assert_eq!(settings.version, 1);
    }

    #[test]
    fn the_narrowest_answer_decides_whether_a_game_has_network() {
        use NetworkAccess::{Allowed, Blocked, Inherit};
        // The game answers for itself, whatever the group and the setting say.
        assert!(resolve_network_access(Allowed, Some(Blocked), false));
        assert!(!resolve_network_access(Blocked, Some(Allowed), true));
        // Otherwise the group answers, and last the global setting.
        assert!(!resolve_network_access(Inherit, Some(Blocked), true));
        assert!(resolve_network_access(Inherit, Some(Inherit), true));
        assert!(!resolve_network_access(Inherit, None, false));
    }

    #[test]
    fn a_folder_named_at_several_levels_keeps_the_narrowest_answer() {
        let folder = |path: &str, writable: bool| SandboxFolder {
            path: path.to_string(),
            writable,
        };
        let resolved = resolve_sandbox_folders(
            &[folder("/games/mods", true)],
            Some(&[folder("/games/shared", false)]),
            &[folder("/games/mods", false), folder("/media/assets", true)],
        );
        assert_eq!(
            resolved,
            vec![
                folder("/games/mods", true),
                folder("/games/shared", false),
                folder("/media/assets", true),
            ]
        );
    }

    #[test]
    fn an_empty_folder_entry_is_dropped() {
        let resolved = resolve_sandbox_folders(
            &[SandboxFolder {
                path: "   ".to_string(),
                writable: true,
            }],
            None,
            &[],
        );
        assert!(resolved.is_empty());
    }

    #[test]
    fn current_version_round_trips() {
        let config = GamesConfig {
            version: GAMES_CONFIG_VERSION,
            items: Vec::new(),
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: GamesConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, GAMES_CONFIG_VERSION);

        let settings = GlobalSettings::default();
        let text = toml::to_string_pretty(&settings).unwrap();
        let parsed: GlobalSettings = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, GLOBAL_SETTINGS_VERSION);
    }
}
