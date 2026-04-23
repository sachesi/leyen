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
    #[serde(default)]
    pub game_wayland: bool,
    #[serde(default)]
    pub game_wow64: bool,
    #[serde(default)]
    pub game_ntsync: bool,
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub cover_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
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
    pub view_mode: ViewMode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GamesConfig {
    pub games: Vec<Game>,
}
