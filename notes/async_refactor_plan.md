# Async Refactor Plan

This plan tracks the remaining tasks to complete the full asynchronous migration of the Leyen launcher.

## Remaining Async Migration Tasks
- [ ] **1. UI Event Handlers (mod.rs):** Convert button click handlers to `glib::spawn_future_local`.
- [ ] **2. Dependency Installation (engine.rs):** Migrate `install_dep_async` and `uninstall_dep_async` to fully async patterns.
- [ ] **3. UI Dialogs (game_dialogs.rs):** Refactor library modification (add/remove game/group) to be async.
- [ ] **4. Settings (settings.rs):** Refactor settings saving to use `save_library_async` / `save_settings_async`.
- [ ] **5. Final Review & Cleanup:** Remove all remaining synchronous I/O fallbacks and unused sync methods.
