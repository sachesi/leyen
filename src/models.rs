use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub exe_path: String,
    pub prefix_path: String,
    pub proton: String,
    pub launch_args: String,
    pub force_mangohud: bool,
    pub force_gamemode: bool,
    pub custom_icon: bool,
    pub game_wayland: bool,
    pub game_wow64: bool,
    pub game_ntsync: bool,
    pub leyen_id: String,
    pub game_id: String,
    pub playtime_seconds: u64,
    pub last_played_epoch_seconds: u64,
    pub last_run_duration_seconds: u64,
    pub last_run_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupLaunchDefaults {
    pub prefix_path: String,
    pub proton: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalSettings {
    pub default_prefix_path: String,
    pub default_proton: String,
    pub global_mangohud: bool,
    pub global_gamemode: bool,
    pub global_wayland: bool,
    pub global_wow64: bool,
    pub global_ntsync: bool,
    pub available_proton_versions: Vec<String>,
    pub log_errors: bool,
    pub log_warnings: bool,
    pub log_operations: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GamesConfig {
    pub items: Vec<LibraryItem>,
}
