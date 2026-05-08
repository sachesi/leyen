use crate::config::get_config_dir;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum InstanceError {
    #[error("Failed to open instance lock file: {0}")]
    OpenError(#[from] std::io::Error),
    #[error("Another instance of Leyen is already running.")]
    AlreadyRunning,
    #[error("Failed to acquire instance lock: {0}")]
    LockError(std::io::Error),
}

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire() -> Result<Self, InstanceError> {
        let lock_path = get_lock_path();

        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)?;

        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result != 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                return Err(InstanceError::AlreadyRunning);
            }
            return Err(InstanceError::LockError(err));
        }

        Ok(Self { _file: file })
    }
}

fn get_lock_path() -> PathBuf {
    get_config_dir().join(".instance.lock")
}

fn get_show_signal_path() -> PathBuf {
    get_config_dir().join(".instance-show")
}

pub fn signal_show_window() -> Result<(), std::io::Error> {
    std::fs::write(get_show_signal_path(), "")
}

pub fn check_and_clear_show_signal() -> bool {
    let signal_path = get_show_signal_path();
    if signal_path.exists() {
        let _ = std::fs::remove_file(&signal_path);
        true
    } else {
        false
    }
}
