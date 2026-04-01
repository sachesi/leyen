# Leyen Codebase Review & Refactor Plan

## Scope
This review compares the current implementation against the feature set documented in `README.md`, then proposes a staged refactor plan.

## What is implemented

### Core app architecture
- The codebase is already modularized into focused modules (`config`, `launch`, `proton`, `deps`, `umu`, and `ui/*`) and no longer keeps all logic in `main.rs`.
- Persistent game and settings models are defined in `src/models.rs` and serialized via TOML.

### Feature implementation status
- **Game CRUD**: add/edit/delete flows exist in UI dialogs and config persistence.
- **Per-game launch controls**: executable path, launch args, prefix, proton, and per-game toggles are supported.
- **Global settings**: default prefix/proton and global toggles are persisted and editable.
- **Proton detection**: scans local/Steam compatibility directories and supports legacy proton value resolution.
- **umu integration**: auto-check/download for `umu-run` is implemented with UI banner state.
- **Dependency manager UI**: dependency dialog and install/uninstall engine are present.
- **Winetricks fallback**: documented behavior aligns with dedicated dependency tooling modules.
- **Logging controls**: runtime log level toggles and in-memory log buffer are implemented.

## What is partially implemented or missing

- **README architecture docs are outdated**: README still says `src/main.rs` contains data structures, I/O, UI, and logic, but code is now split across modules.
- **Environment variable management UI**: README claims generic environment variable management, but UI currently supports launch-arg token parsing and specific toggles rather than a structured key/value editor.
- **Validation/error surfacing**: file I/O and command operations often swallow errors (`let _ = ...`) and rely on silent fallback behavior.
- **Path handling consistency**: path construction patterns were duplicated across modules and mixed `String` formatting with `PathBuf`.
- **Automated test coverage**: no tests are present for config migration/serialization, proton resolution, or command construction.

## Refactor plan (phased)

### Phase 1 — Foundations (low risk)
1. Centralize common filesystem root/path helpers (config dir, local share dir, steam root).
2. Replace manual string path building with `PathBuf` joins.
3. Add small unit tests for path and proton resolution helpers.

### Phase 2 — Reliability
1. Introduce typed error handling (`thiserror` or similar) for config + launcher operations.
2. Propagate recoverable errors to UI toasts/logging instead of silent ignore.
3. Add pre-launch validation (missing executable, invalid prefix, missing proton path).

### Phase 3 — UI/UX consistency
1. Extract reusable row/builders for add/edit dialogs to remove duplicated widget wiring.
2. Add a structured environment-variable editor (key/value list) to match README promise.
3. Improve settings/save UX with explicit save success/failure notifications.

### Phase 4 — Testability
1. Add unit tests for:
   - load/save settings and backward compatibility defaults
   - proton path resolution behavior
   - launch argument parsing around `%command%`
2. Add smoke integration tests for config read/write in temporary directories.

## Immediate next tasks recommended
1. Update README architecture section to reflect current modular layout.
2. Track feature parity gaps as issues (env var editor, validation, tests).
3. Continue path/error handling cleanups before larger UI refactors.
