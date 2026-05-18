# Code Audit & Optimization Report

**Summary:**
- Critical Bugs: 3 [DONE]
- High Bugs: 1 [DONE]
- High Performance/Stability: 1 [DONE]
- Medium Performance: 2 [DONE]
- Total Estimated Effort/Risk: Medium effort, low risk.

---

### [Critical] – Category: Bug (State Inconsistency / Concurrency) [DONE]
**File:** `src/config.rs`
**Lines:** ~130–145, 172-192

**Description:**
The application used a file lock when writing, but `load_library` did not acquire it. Atomic rename pattern was missing.

**Current Behavior:**
Concurrent reads and writes to `games.toml` could result in data loss during truncation.

**Proposed Fix:**
Use atomic file writes (write to a temporary file, then rename over the original). [IMPLEMENTED]

---

### [Critical] – Category: Bug (Silent Data Loss) [DONE]
**File:** `src/config.rs`
**Lines:** ~132-152 (`load_library`)

**Description:**
`load_library` returned `Vec::new()` on any error, which could cause the UI to show an empty library and subsequently overwrite the real config with an empty one.

**Proposed Fix:**
Change `load_library` to return `Result<Vec<LibraryItem>, String>` and handle it in all 16+ callers. [IMPLEMENTED]

---

### [Critical] – Category: Bug (State Inconsistency / Concurrency) [DONE]
**File:** `src/config.rs`
**Lines:** ~185–205 (`load_settings` and `save_settings`)

**Description:**
`settings.toml` lacked locking and atomic writes.

**Proposed Fix:**
Implement atomic write pattern with a temporary file and `fs::rename`. [IMPLEMENTED]

---

### [High] – Category: Performance [DONE]
**File:** `src/launch.rs`
**Lines:** ~540–580 (`scan_all_procs`, `read_parent_pid`, `is_game_process`)

**Description:**
Massive number of string allocations and `read_to_string` calls during `/proc` polling.

**Proposed Fix:**
1. Avoid `read_to_string`. Use `File::open` and `Read::read` into stack-allocated buffers. [IMPLEMENTED]
2. Use `to_ascii_lowercase()` for command names. [IMPLEMENTED]

---

### [High] – Category: Performance / Stability [DONE]
**File:** `src/icons.rs`
**Lines:** ~121-140 (`extract_best_icon_to_png`)

**Description:**
`fs::read(exe_path)` loaded entire executables (often >1GB) into memory for icon extraction.

**Proposed Fix:**
Cap the read to the first 16MB for large files, which typically contains the PE headers and resource sections needed for icon extraction. [IMPLEMENTED]

---

### [Medium] – Category: Performance [DONE]
**File:** `src/deps/engine.rs`
**Lines:** ~800-850 (`collect_snapshot`)

**Description:**
WINE prefix scanning followed redundant symlinks in `dosdevices` and used `metadata()` excessively.

**Proposed Fix:**
1. Use `file_type()` instead of `metadata()` for directory checks.
2. Skip the `dosdevices` directory to avoid redundant walks. [IMPLEMENTED]

---

### [Medium] – Category: Performance / UI Responsiveness [DONE]
**File:** `src/ui/library/mod.rs`

**Description:**
Rebuilding UI widgets on every refresh.

**Current Behavior:**
The application already uses a double-buffering (swapping list boxes in a stack) mechanism to prevent flickering. This is considered sufficient for current library sizes. [VERIFIED]
