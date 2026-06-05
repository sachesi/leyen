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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
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

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GamesConfig {
    pub items: Vec<LibraryItem>,
}
