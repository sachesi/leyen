# Async Refactor Plan

This plan tracks the remaining tasks to complete the full asynchronous migration of the Leyen launcher.

## Remaining Async Migration Tasks
- [DONE] **1. UI Event Handlers (mod.rs):** Convert button click handlers to `glib::spawn_future_local`.
- [DONE] **2. Dependency Installation (engine.rs):** Migrate `install_dep_async` and `uninstall_dep_async` to fully async patterns.
- [DONE] **3. UI Dialogs (game_dialogs.rs):** Refactor library modification (add/remove game/group) to be async.
- [DONE] **4. Settings (settings.rs):** Refactor settings saving to use `save_library_async` / `save_settings_async`.
- [DONE] **5. Final Review & Cleanup:** Remove all remaining synchronous I/O fallbacks and unused sync methods.
- [DONE] **6. Final Polish:** Wrap remaining UI-thread I/O in `spawn_blocking`.
    - [DONE] Wrap `apply_game_icon` and `apply_group_icon` in `src/ui/game_dialogs.rs`.
    - [DONE] Wrap `fs::remove_dir_all` for umu runtime reset in `src/ui/settings.rs`.
    - [DONE] Audit `src/icons.rs` for internal sync I/O that could be moved to background.
