use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn config_dir() -> PathBuf {
    home_dir().join(".config/leyen")
}

pub fn local_share_leyen_dir() -> PathBuf {
    home_dir().join(".local/share/leyen")
}

pub fn steam_root_dir() -> PathBuf {
    home_dir().join(".steam/steam")
}
