use serde::{Deserialize, Serialize};

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
    pub gamemode: bool,
    pub wayland: bool,
    pub wow64: bool,
    pub ntsync: bool,
    pub hdr: bool,
    pub proton_log: bool,
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
    pub global_gamemode: bool,
    pub global_wayland: bool,
    pub global_wow64: bool,
    pub global_ntsync: bool,
    pub global_hdr: bool,
    pub global_proton_log: bool,
    pub available_proton_versions: Vec<String>,
    pub log_errors: bool,
    pub log_warnings: bool,
    pub log_operations: bool,
    /// When a game launches while another sharing its Wine prefix is already
    /// running, run it inside the existing pressure-vessel container
    /// (`UMU_CONTAINER_NSENTER`). Disable to launch it in its own container on the
    /// same prefix instead. Defaults to enabled to preserve prior behavior.
    #[serde(default = "default_true")]
    pub use_shared_container: bool,
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
            global_gamemode: false,
            global_wayland: false,
            global_wow64: false,
            global_ntsync: false,
            global_hdr: false,
            global_proton_log: false,
            available_proton_versions: Vec::new(),
            log_errors: true,
            log_warnings: true,
            log_operations: true,
            use_shared_container: true,
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
