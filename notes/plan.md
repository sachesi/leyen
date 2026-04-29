# Leyen Project Plan

## 1. Path Management - [DONE]
Currently, the project relies on the `HOME` environment variable and hardcoded strings for paths (e.g., `~/.config/leyen`, `~/.local/share/leyen`).
- **Proposed Change**: Integrate the `directories` crate to handle project directories in a cross-platform and standard-compliant manner (`ProjectDirs`). This ensures better compatibility with XDG specifications.
- **Status**: Completed. Used `ProjectDirs` for config and data directories.

## 2. Robust CLI Implementation - [DONE]
The CLI is currently manually parsed in `src/cli.rs`, which is difficult to maintain and lacks standard features like automatic help generation.
- **Proposed Change**: Switch to `clap` (Command Line Argument Parser). This would simplify argument handling, provide professional help text, and allow for easier addition of new commands.
- **Status**: Completed. Switched to `clap` with subcommands and automatic help.

## 3. Error Handling - [DONE]
The project uses `Result<(), String>` for most error-prone operations, and often silently ignores errors with `let _ = ...`.
- **Proposed Change**: Use `anyhow` for top-level error handling and `thiserror` for more structured errors in internal modules. This will provide better context and easier debugging.
- **Status**: Completed. `anyhow` used in `cli` and `main`, `thiserror` used in `launch`, `umu`, and `instance`.

## 4. Logging Facade - [DONE]
Logging is implemented as a custom JSONL writer with low-level file locking (`libc::flock`).
- **Proposed Change**: Adopt a standard logging facade like `log` or `tracing`. The existing persistence logic can be moved into a custom logger implementation if the JSONL format is still required for the UI logs window.
- **Status**: Completed. Implemented `log::Log` for `LeyenLogger` while maintaining JSONL persistence and file locking.

## 5. UI Definition - [SKIP FOR NOW]
The UI is constructed entirely in Rust code, which can become verbose and hard to visualize.
- **Proposed Change**: Consider using **Blueprint** for defining GTK4/Libadwaita interfaces. This separates the UI layout from the logic and makes it easier to design and maintain the interface.

## 6. Dependency Management - [DONE]
Some paths and logic for Proton and umu-launcher detection are scattered.
- **Proposed Change**: Centralize these into a dedicated `discovery` or `runtime` module to make the logic more reusable across the CLI and UI.
- **Status**: Completed. Centralized Proton and umu-launcher logic into `src/runtime/`.

## 7. Async/Non-blocking Operations - [DONE]
GTK applications should avoid blocking the main thread.
- **Proposed Change**: Ensure all I/O bound operations (like `check_or_install_umu` or large file reads) are handled asynchronously or in separate threads using `glib::MainContext::spawn_local` or similar patterns to keep the UI responsive.
- **Status**: Completed. Converted umu download thread to use `glib::spawn_future_local` and `tokio::task::spawn_blocking`.

## 8. Single Instance Enforcement - [DONE]
Currently, multiple instances of Leyen can be started simultaneously, which might lead to race conditions when writing to the same configuration or log files.
- **Proposed Change**: Implement a single-instance check (e.g., using a PID file or a platform-specific IPC mechanism like `GtkApplication`'s built-in support) to ensure only one instance is active.
- **Status**: Completed. Implemented `InstanceLock` using `flock` in `src/instance.rs`.

## 9. umu-launcher Update Mechanism - [DONE]
The current `check_or_install_umu` logic only checks if `umu-run` is present. It does not check for updates if a version is already installed locally.
- **Proposed Change**: Implement a version check or a periodic update check for the local `umu-launcher` installation to ensure the user has the latest bug fixes and features.
- **Status**: Completed. Added `get_local_umu_version` and `check_for_umu_updates` in `src/umu.rs`.

## 10. Proton Discovery Logic - [DONE]
The application scans `~/.steam/steam/steamapps/common` for directories containing "Proton". While common, this might include incomplete or non-functional Proton installations.
- **Proposed Change**: Refine Proton discovery to prioritize `compatibilitytools.d` and perhaps use a more robust detection method (like checking for the existence of `proton` and `version` files) rather than just directory name matching in `common`.
- **Status**: Completed. Refined Proton discovery to require `proton` and `version` files.

## 11. Single Instance for Utility Windows - [DONE]
Currently, multiple instances of the "Logs" and "Running Games" windows can be opened simultaneously.
- **Proposed Change**: Ensure that only one instance of each utility window can be active at a time. If the user tries to open it again, focus the existing window instead of creating a new one.
- **Status**: Completed. Implemented `thread_local!` single-instance tracking for utility windows.

## 12. Dependency Manager Verification - [DONE]
Leyen includes a custom dependency manager for installing Windows redistributables and Wine components.
- **Proposed Change**: Conduct a thorough audit of the dependency manager's logic for correctness, including error handling during downloads, installation verification, and proper state tracking in the Wine prefix.
- **Status**: Completed. Implemented SHA256 checksum verification, post-install registry verification, and reboot code (3010) handling.
