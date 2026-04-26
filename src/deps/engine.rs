use libadwaita as adw;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, UNIX_EPOCH};

use gtk4::glib;

use crate::logging::{LOG_OPERATIONS, leyen_log};
use crate::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

use super::catalog::{DepProfile, get_dep_profile, get_dep_steps};
use super::{
    InstalledDependency, find_installed_dependents, get_deps_cache_dir, read_prefix_dep_state,
    remove_installed_dep, upsert_installed_dep,
};

#[derive(Clone)]
#[allow(dead_code)]
pub enum DepStepAction {
    DownloadFile {
        url: &'static str,
        file_name: &'static str,
    },
    RunExe {
        file_name: &'static str,
        args: &'static str,
        extra_env: &'static str,
    },
    RunMsi {
        file_name: &'static str,
        args: &'static str,
    },
    OverrideDlls {
        dlls: &'static str,
        override_type: &'static str,
    },
    RegisterDll {
        dll: &'static str,
    },
    ExtractArchive {
        archive_name: &'static str,
        dest_subdir: &'static str,
    },
    CopyDllsToPrefix {
        src_subdir: &'static str,
        dlls: &'static str,
        wine_dir: &'static str,
    },
    RunWinetricks {
        verb: String,
    },
}

#[derive(Clone)]
pub struct DepStep {
    pub description: &'static str,
    pub action: DepStepAction,
}

pub enum DepInstallMsg {
    Progress {
        step: usize,
        total: usize,
        description: String,
    },
    Done(Option<String>),
    Failed(String),
}

#[derive(Default)]
struct StepChanges {
    created_files: Vec<String>,
    touched_existing_files: bool,
    dll_overrides: Vec<String>,
    registered_dlls: Vec<String>,
}

impl StepChanges {
    fn merge(&mut self, other: StepChanges) {
        self.created_files = merge_unique_strings(&self.created_files, &other.created_files);
        self.touched_existing_files |= other.touched_existing_files;
        self.dll_overrides = merge_unique_strings(&self.dll_overrides, &other.dll_overrides);
        self.registered_dlls = merge_unique_strings(&self.registered_dlls, &other.registered_dlls);
    }

    fn into_dependency_record(self) -> InstalledDependency {
        InstalledDependency {
            created_files: self.created_files,
            touched_existing_files: self.touched_existing_files,
            dll_overrides: self.dll_overrides,
            registered_dlls: self.registered_dlls,
            ..InstalledDependency::default()
        }
    }
}

#[derive(Default)]
struct PrefixSnapshot {
    files: BTreeMap<String, FileFingerprint>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified_epoch_seconds: u64,
}

enum CleanupAction {
    RemoveDllOverrides(Vec<String>),
    UnregisterDlls(Vec<String>),
    RemoveCreatedFiles(Vec<String>),
}

pub const DEP_ASYNC_POLL_MS: u64 = 50;

fn execute_dep_step(
    step: &DepStep,
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
) -> Result<StepChanges, String> {
    match &step.action {
        DepStepAction::DownloadFile { url, file_name } => {
            if !url.starts_with("https://") {
                return Err(format!(
                    "Refusing to download '{}' from a non-HTTPS source",
                    file_name
                ));
            }

            let dest = Path::new(cache_dir).join(file_name);
            if dest.exists() {
                return Ok(StepChanges::default());
            }

            fs::create_dir_all(cache_dir)
                .map_err(|err| format!("Failed to create dependency cache directory: {err}"))?;

            let status = std::process::Command::new("curl")
                .args([
                    "--proto",
                    "=https",
                    "--tlsv1.2",
                    "--silent",
                    "--show-error",
                    "--fail",
                    "--location",
                    "--retry",
                    "3",
                    "--retry-delay",
                    "1",
                    "-o",
                    dest.to_string_lossy().as_ref(),
                    url,
                ])
                .status()
                .map_err(|err| format!("curl unavailable: {err}"))?;

            if !status.success() {
                let _ = fs::remove_file(&dest);
                return Err(format!("Download failed for {}", file_name));
            }

            Ok(StepChanges::default())
        }

        DepStepAction::RunExe {
            file_name,
            args,
            extra_env,
        } => {
            let exe_path = Path::new(cache_dir).join(file_name);
            let before = snapshot_prefix(prefix_path)?;

            let mut cmd = std::process::Command::new(get_umu_run_path());
            configure_umu_command(&mut cmd, prefix_path, proton_path);
            for pair in extra_env.split_whitespace() {
                if let Some(eq) = pair.find('=') {
                    cmd.env(&pair[..eq], &pair[eq + 1..]);
                }
            }
            cmd.arg(exe_path.as_os_str());
            for arg in args.split_whitespace() {
                cmd.arg(arg);
            }
            maybe_silence_command(&mut cmd);

            let status = cmd
                .status()
                .map_err(|err| format!("Failed to launch {}: {}", file_name, err))?;
            if !status.success() {
                return Err(format!("Installer '{}' exited with an error", file_name));
            }

            let after = snapshot_prefix(prefix_path)?;
            Ok(diff_snapshots(&before, &after))
        }

        DepStepAction::RunMsi { file_name, args } => {
            let msi_path = Path::new(cache_dir).join(file_name);
            let before = snapshot_prefix(prefix_path)?;

            let mut cmd = std::process::Command::new(get_umu_run_path());
            configure_umu_command(&mut cmd, prefix_path, proton_path);
            cmd.args(["msiexec.exe", "/i"]);
            cmd.arg(msi_path.as_os_str());
            for arg in args.split_whitespace() {
                cmd.arg(arg);
            }
            maybe_silence_command(&mut cmd);

            let status = cmd
                .status()
                .map_err(|err| format!("Failed to run msiexec for {}: {}", file_name, err))?;
            if !status.success() {
                return Err(format!("MSI install '{}' failed", file_name));
            }

            let after = snapshot_prefix(prefix_path)?;
            Ok(diff_snapshots(&before, &after))
        }

        DepStepAction::OverrideDlls { dlls, .. } => Ok(StepChanges {
            dll_overrides: split_csv_values(dlls),
            ..StepChanges::default()
        }),

        DepStepAction::RegisterDll { dll } => Ok(StepChanges {
            registered_dlls: vec![dll.to_string()],
            ..StepChanges::default()
        }),

        DepStepAction::ExtractArchive {
            archive_name,
            dest_subdir,
        } => {
            let archive_path = Path::new(cache_dir).join(archive_name);
            let dest_path = Path::new(cache_dir).join(dest_subdir);
            fs::create_dir_all(&dest_path).map_err(|err| {
                format!(
                    "Failed to create extraction directory '{}': {}",
                    dest_path.display(),
                    err
                )
            })?;

            let mut cmd = std::process::Command::new("tar");
            if archive_name.ends_with(".tar.zst") || archive_name.ends_with(".tzst") {
                cmd.args(["-I", "zstd", "-xf"]);
            } else {
                cmd.arg("-xf");
            }
            cmd.arg(archive_path.as_os_str());
            cmd.args(["-C"]);
            cmd.arg(dest_path.as_os_str());
            cmd.args(["--strip-components=1"]);

            let status = cmd
                .status()
                .map_err(|err| format!("tar unavailable: {err}"))?;
            if !status.success() {
                return Err(format!("Failed to extract '{}'", archive_name));
            }

            Ok(StepChanges::default())
        }

        DepStepAction::CopyDllsToPrefix {
            src_subdir,
            dlls,
            wine_dir,
        } => {
            let src_dir = Path::new(cache_dir).join(src_subdir);
            let dst_dir = Path::new(prefix_path)
                .join("drive_c")
                .join("windows")
                .join(wine_dir);
            fs::create_dir_all(&dst_dir).map_err(|err| {
                format!(
                    "Failed to create target directory '{}': {}",
                    dst_dir.display(),
                    err
                )
            })?;

            let mut changes = StepChanges::default();
            for dll in split_csv_values(dlls) {
                let src = src_dir.join(format!("{dll}.dll"));
                let dst = dst_dir.join(format!("{dll}.dll"));
                let existed_before = dst.exists();

                fs::copy(&src, &dst)
                    .map_err(|err| format!("Failed to copy {}.dll: {}", dll, err))?;

                if existed_before {
                    changes.touched_existing_files = true;
                } else if let Some(relative) = path_to_prefix_relative(Path::new(prefix_path), &dst)
                {
                    changes.created_files.push(relative);
                }
            }

            changes.created_files = merge_unique_strings(&[], &changes.created_files);
            Ok(changes)
        }

        DepStepAction::RunWinetricks { verb } => {
            let before = snapshot_prefix(prefix_path)?;

            let mut cmd = std::process::Command::new(get_umu_run_path());
            configure_umu_command(&mut cmd, prefix_path, proton_path);
            cmd.args(["winetricks", "-q", verb.as_str()]);
            maybe_silence_command(&mut cmd);

            let status = cmd
                .status()
                .map_err(|err| format!("Failed to run winetricks {}: {}", verb, err))?;
            if !status.success() {
                return Err(format!("winetricks '{}' failed", verb));
            }

            let after = snapshot_prefix(prefix_path)?;
            Ok(diff_snapshots(&before, &after))
        }
    }
}

pub fn install_dep_async(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
    on_progress: impl Fn(usize, usize, String) + 'static,
    on_finish: impl FnOnce(bool, Option<String>) + 'static,
) {
    if let Err(message) = ensure_umu_ready(overlay) {
        on_finish(false, Some(message));
        return;
    }

    let state = read_prefix_dep_state(prefix_path);
    let install_plan = match build_install_plan(dep_id, &state) {
        Ok(plan) if !plan.is_empty() => plan,
        Ok(_) => {
            on_finish(true, Some("Dependency is already installed.".to_string()));
            return;
        }
        Err(message) => {
            overlay.add_toast(adw::Toast::new(&message));
            on_finish(false, Some(message));
            return;
        }
    };

    let total_steps = install_plan
        .iter()
        .map(|profile| get_dep_steps(profile.id).len())
        .sum::<usize>();
    if total_steps == 0 {
        let message = format!("No install steps defined for '{}'", dep_id);
        overlay.add_toast(adw::Toast::new(&message));
        on_finish(false, Some(message));
        return;
    }

    let queue = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::<DepInstallMsg>::new()));
    let queue_bg = queue.clone();

    let on_finish = std::rc::Rc::new(std::cell::RefCell::new(Some(on_finish)));
    let on_progress = std::rc::Rc::new(on_progress);

    glib::timeout_add_local(Duration::from_millis(DEP_ASYNC_POLL_MS), move || {
        let mut q = queue.lock().unwrap();
        while let Some(msg) = q.pop_front() {
            match msg {
                DepInstallMsg::Progress {
                    step,
                    total,
                    description,
                } => {
                    on_progress(step, total, description);
                }
                DepInstallMsg::Done(note) => {
                    if let Some(finish) = on_finish.borrow_mut().take() {
                        finish(true, note);
                    }
                    return glib::ControlFlow::Break;
                }
                DepInstallMsg::Failed(error) => {
                    if let Some(finish) = on_finish.borrow_mut().take() {
                        finish(false, Some(error));
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });

    let dep_id = dep_id.to_string();
    let prefix_path = prefix_path.to_string();
    let proton_path = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();
    std::thread::spawn(move || {
        leyen_log(
            "INFO ",
            &format!(
                "[dep:{}] starting install plan ({} profiles, {} steps)",
                dep_id,
                install_plan.len(),
                total_steps
            ),
        );

        let mut completed_steps = 0usize;
        for profile in &install_plan {
            let steps = get_dep_steps(profile.id);
            let mut recorded = StepChanges::default();

            for step in &steps {
                completed_steps += 1;
                let description = if install_plan.len() > 1 {
                    format!("{}: {}", profile.name, step.description)
                } else {
                    step.description.to_string()
                };

                leyen_log(
                    "INFO ",
                    &format!(
                        "[dep:{}] step {}/{}: {}",
                        profile.id, completed_steps, total_steps, description
                    ),
                );
                queue_bg.lock().unwrap().push_back(DepInstallMsg::Progress {
                    step: completed_steps,
                    total: total_steps,
                    description,
                });

                match execute_dep_step(step, &prefix_path, &proton_path, &cache_dir) {
                    Ok(changes) => recorded.merge(changes),
                    Err(error) => {
                        leyen_log(
                            "ERROR",
                            &format!("[dep:{}] install failed: {}", profile.id, error),
                        );
                        queue_bg
                            .lock()
                            .unwrap()
                            .push_back(DepInstallMsg::Failed(error));
                        return;
                    }
                }
            }

            if let Err(error) = upsert_installed_dep(
                &prefix_path,
                profile.id,
                profile.dependencies,
                &recorded.into_dependency_record(),
            ) {
                queue_bg
                    .lock()
                    .unwrap()
                    .push_back(DepInstallMsg::Failed(error));
                return;
            }
        }

        let note = if install_plan.len() > 1 {
            let prerequisites = install_plan
                .iter()
                .map(|profile| profile.id)
                .filter(|profile_id| *profile_id != dep_id)
                .collect::<Vec<_>>();
            if prerequisites.is_empty() {
                None
            } else {
                Some(format!(
                    "Installed prerequisites: {}.",
                    prerequisites.join(", ")
                ))
            }
        } else {
            None
        };

        leyen_log("INFO ", &format!("[dep:{}] install complete", dep_id));
        queue_bg
            .lock()
            .unwrap()
            .push_back(DepInstallMsg::Done(note));
    });
}

pub fn uninstall_dep_async(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
    on_progress: impl Fn(usize, usize, String) + 'static,
    on_finish: impl FnOnce(bool, Option<String>) + 'static,
) {
    let state = read_prefix_dep_state(prefix_path);
    let installed = match state.installed.get(dep_id).cloned() {
        Some(installed) => installed,
        None => {
            on_finish(true, Some("Dependency is no longer tracked.".to_string()));
            return;
        }
    };
    let dependents = find_installed_dependents(&state, dep_id);
    if !dependents.is_empty() {
        on_finish(
            false,
            Some(format!(
                "Cannot remove '{}': still required by {}.",
                dep_id,
                dependents.join(", ")
            )),
        );
        return;
    }

    let actions = build_cleanup_actions(&installed);
    let requires_umu = actions.iter().any(|(_, action)| {
        matches!(
            action,
            CleanupAction::RemoveDllOverrides(_) | CleanupAction::UnregisterDlls(_)
        )
    });
    if requires_umu && let Err(message) = ensure_umu_ready(overlay) {
        on_finish(false, Some(message));
        return;
    }

    let queue = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::<DepInstallMsg>::new()));
    let queue_bg = queue.clone();

    let on_finish = std::rc::Rc::new(std::cell::RefCell::new(Some(on_finish)));
    let on_progress = std::rc::Rc::new(on_progress);

    glib::timeout_add_local(Duration::from_millis(DEP_ASYNC_POLL_MS), move || {
        let mut q = queue.lock().unwrap();
        while let Some(msg) = q.pop_front() {
            match msg {
                DepInstallMsg::Progress {
                    step,
                    total,
                    description,
                } => {
                    on_progress(step, total, description);
                }
                DepInstallMsg::Done(note) => {
                    if let Some(finish) = on_finish.borrow_mut().take() {
                        finish(true, note);
                    }
                    return glib::ControlFlow::Break;
                }
                DepInstallMsg::Failed(error) => {
                    if let Some(finish) = on_finish.borrow_mut().take() {
                        finish(false, Some(error));
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });

    let dep_id = dep_id.to_string();
    let prefix_path = prefix_path.to_string();
    let proton_path = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();
    std::thread::spawn(move || {
        leyen_log(
            "INFO ",
            &format!(
                "[dep:{}] starting removal ({} cleanup actions)",
                dep_id,
                actions.len()
            ),
        );

        for (index, (description, action)) in actions.iter().enumerate() {
            queue_bg.lock().unwrap().push_back(DepInstallMsg::Progress {
                step: index + 1,
                total: actions.len(),
                description: description.clone(),
            });

            let result = match action {
                CleanupAction::RemoveDllOverrides(dlls) => {
                    remove_dll_overrides(&prefix_path, &proton_path, &cache_dir, dlls)
                }
                CleanupAction::UnregisterDlls(dlls) => {
                    unregister_dlls(&prefix_path, &proton_path, dlls)
                }
                CleanupAction::RemoveCreatedFiles(files) => {
                    remove_created_files(&prefix_path, files)
                }
            };

            if let Err(error) = result {
                leyen_log(
                    "ERROR",
                    &format!("[dep:{}] removal failed: {}", dep_id, error),
                );
                queue_bg
                    .lock()
                    .unwrap()
                    .push_back(DepInstallMsg::Failed(error));
                return;
            }
        }

        if let Err(error) = remove_installed_dep(&prefix_path, &dep_id) {
            queue_bg
                .lock()
                .unwrap()
                .push_back(DepInstallMsg::Failed(error));
            return;
        }

        let note = match (installed.has_removable_changes(), installed.touched_existing_files) {
            (true, true) => Some(
                "Some existing prefix files were changed during installation and were not reverted."
                    .to_string(),
            ),
            (false, true) => Some(
                "Removed from tracking. Existing prefix files changed during installation were not reverted.".to_string(),
            ),
            (false, false) => Some("Removed from tracking.".to_string()),
            (true, false) => None,
        };

        leyen_log("INFO ", &format!("[dep:{}] removal complete", dep_id));
        queue_bg
            .lock()
            .unwrap()
            .push_back(DepInstallMsg::Done(note));
    });
}

fn ensure_umu_ready(overlay: &adw::ToastOverlay) -> Result<(), String> {
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return Err("umu-launcher not ready".to_string());
    }

    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return Err("umu-launcher not available".to_string());
    }

    Ok(())
}

fn configure_umu_command(cmd: &mut std::process::Command, prefix_path: &str, proton_path: &str) {
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", "leyen-dep-install");
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
}

fn maybe_silence_command(cmd: &mut std::process::Command) {
    if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
}

fn build_install_plan(
    dep_id: &str,
    state: &super::PrefixDependencyState,
) -> Result<Vec<&'static DepProfile>, String> {
    let mut visiting = BTreeSet::new();
    let mut planned_ids = BTreeSet::new();
    let mut planned = Vec::new();
    append_install_plan(
        dep_id,
        state,
        &mut visiting,
        &mut planned_ids,
        &mut planned,
        true,
    )?;
    Ok(planned)
}

fn append_install_plan(
    dep_id: &str,
    state: &super::PrefixDependencyState,
    visiting: &mut BTreeSet<String>,
    planned_ids: &mut BTreeSet<String>,
    planned: &mut Vec<&'static DepProfile>,
    include_even_if_installed: bool,
) -> Result<(), String> {
    if planned_ids.contains(dep_id) {
        return Ok(());
    }

    if !visiting.insert(dep_id.to_string()) {
        return Err(format!("Dependency cycle detected for '{}'", dep_id));
    }

    let profile = get_dep_profile(dep_id)
        .ok_or_else(|| format!("No dependency profile found for '{}'", dep_id))?;

    for dependency in profile.dependencies {
        append_install_plan(dependency, state, visiting, planned_ids, planned, false)?;
    }

    let should_add = include_even_if_installed || !state.installed.contains_key(dep_id);
    if should_add && planned_ids.insert(dep_id.to_string()) {
        planned.push(profile);
    }

    visiting.remove(dep_id);
    Ok(())
}

fn split_csv_values(values: &str) -> Vec<String> {
    values
        .split(',')
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .collect()
}

fn merge_unique_strings(existing: &[String], additional: &[String]) -> Vec<String> {
    let mut merged: BTreeSet<String> = existing.iter().cloned().collect();
    merged.extend(additional.iter().cloned());
    merged.into_iter().collect()
}

fn snapshot_prefix(prefix_path: &str) -> Result<PrefixSnapshot, String> {
    let root = Path::new(prefix_path);
    if !root.exists() {
        return Ok(PrefixSnapshot::default());
    }

    let mut snapshot = PrefixSnapshot::default();
    collect_snapshot(root, root, &mut snapshot)?;
    Ok(snapshot)
}

fn collect_snapshot(
    root: &Path,
    current: &Path,
    snapshot: &mut PrefixSnapshot,
) -> Result<(), String> {
    let entries = fs::read_dir(current).map_err(|err| {
        format!(
            "Failed to read prefix directory '{}': {}",
            current.display(),
            err
        )
    })?;

    for entry in entries {
        let entry =
            entry.map_err(|err| format!("Failed to read prefix directory entry: {}", err))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|err| format!("Failed to read metadata for '{}': {}", path.display(), err))?;

        if metadata.is_dir() {
            collect_snapshot(root, &path, snapshot)?;
            continue;
        }

        if !metadata.is_file() {
            continue;
        }

        if let Some(relative) = path_to_prefix_relative(root, &path) {
            snapshot.files.insert(
                relative,
                FileFingerprint {
                    len: metadata.len(),
                    modified_epoch_seconds: metadata
                        .modified()
                        .ok()
                        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                        .map(|value| value.as_secs())
                        .unwrap_or(0),
                },
            );
        }
    }

    Ok(())
}

fn diff_snapshots(before: &PrefixSnapshot, after: &PrefixSnapshot) -> StepChanges {
    let mut changes = StepChanges::default();

    for (path, fingerprint) in &after.files {
        match before.files.get(path) {
            None => changes.created_files.push(path.clone()),
            Some(previous) if previous != fingerprint => changes.touched_existing_files = true,
            _ => {}
        }
    }

    if before
        .files
        .keys()
        .any(|path| !after.files.contains_key(path))
    {
        changes.touched_existing_files = true;
    }

    changes.created_files.sort();
    changes.created_files.dedup();
    changes
}

fn path_to_prefix_relative(prefix_root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(prefix_root)
        .ok()
        .map(|relative| {
            relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|relative| !relative.is_empty())
}

fn build_cleanup_actions(installed: &InstalledDependency) -> Vec<(String, CleanupAction)> {
    let mut actions = Vec::new();

    if !installed.dll_overrides.is_empty() {
        actions.push((
            "Removing tracked DLL overrides…".to_string(),
            CleanupAction::RemoveDllOverrides(installed.dll_overrides.clone()),
        ));
    }

    if !installed.registered_dlls.is_empty() {
        actions.push((
            "Unregistering tracked DLLs…".to_string(),
            CleanupAction::UnregisterDlls(installed.registered_dlls.clone()),
        ));
    }

    if !installed.created_files.is_empty() {
        actions.push((
            format!("Removing {} tracked files…", installed.created_files.len()),
            CleanupAction::RemoveCreatedFiles(installed.created_files.clone()),
        ));
    }

    actions
}

fn remove_dll_overrides(
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
    dlls: &[String],
) -> Result<(), String> {
    fs::create_dir_all(cache_dir)
        .map_err(|err| format!("Failed to create dependency cache directory: {err}"))?;

    let reg_lines = dlls
        .iter()
        .map(|dll| format!("\"{}\"=-", dll))
        .collect::<Vec<_>>();
    let reg_content = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n\
         [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
         {}\r\n",
        reg_lines.join("\r\n")
    );

    let reg_path = Path::new(cache_dir).join("remove_dependency_overrides.reg");
    fs::write(&reg_path, reg_content)
        .map_err(|err| format!("Failed to write override removal file: {err}"))?;

    let mut cmd = std::process::Command::new(get_umu_run_path());
    configure_umu_command(&mut cmd, prefix_path, proton_path);
    cmd.args(["regedit.exe", "/S"]);
    cmd.arg(reg_path.as_os_str());
    maybe_silence_command(&mut cmd);

    let status = cmd
        .status()
        .map_err(|err| format!("Failed to run regedit: {err}"))?;
    let _ = fs::remove_file(&reg_path);

    if !status.success() {
        return Err("Failed to remove DLL overrides".to_string());
    }

    Ok(())
}

fn unregister_dlls(prefix_path: &str, proton_path: &str, dlls: &[String]) -> Result<(), String> {
    for dll in dlls {
        let mut cmd = std::process::Command::new(get_umu_run_path());
        configure_umu_command(&mut cmd, prefix_path, proton_path);
        cmd.args(["regsvr32.exe", "/u", "/s", dll]);
        maybe_silence_command(&mut cmd);

        let status = cmd
            .status()
            .map_err(|err| format!("Failed to run regsvr32 for '{}': {}", dll, err))?;
        if !status.success() {
            return Err(format!("Failed to unregister '{}'", dll));
        }
    }

    Ok(())
}

fn remove_created_files(prefix_path: &str, files: &[String]) -> Result<(), String> {
    let prefix_root = Path::new(prefix_path);
    let mut files = files.to_vec();
    files.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));

    for relative in &files {
        let path = prefix_root.join(relative);
        match fs::remove_file(&path) {
            Ok(()) => prune_empty_parent_dirs(prefix_root, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "Failed to remove tracked file '{}': {}",
                    path.display(),
                    err
                ));
            }
        }
    }

    Ok(())
}

fn prune_empty_parent_dirs(prefix_root: &Path, file_path: &Path) {
    let mut current = file_path.parent().map(PathBuf::from);
    while let Some(path) = current {
        if path == prefix_root {
            break;
        }

        match fs::remove_dir(&path) {
            Ok(()) => current = path.parent().map(PathBuf::from),
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                current = path.parent().map(PathBuf::from)
            }
            Err(_) => break,
        }
    }
}
