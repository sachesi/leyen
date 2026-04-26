use std::path::Path;

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn command_available(command: &str) -> bool {
    if command.contains('/') || command.trim().is_empty() {
        return false;
    }

    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(command)))
}

pub fn mangohud_available() -> bool {
    command_available("mangohud")
}

pub fn gamemode_available() -> bool {
    command_available("gamemoderun")
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_command_is_not_available() {
        assert!(!super::command_available(""));
    }
}
