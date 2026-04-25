use libadwaita as adw;

use std::fs;
use std::process::Stdio;

use gtk4::glib;

use crate::logging::{LOG_OPERATIONS, leyen_log};
use crate::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

use super::catalog::{get_dep_steps, get_dep_uninstall_steps};
use super::get_deps_cache_dir;

// ── Data Structures ──────────────────────────────────────────────────────────

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
    /// Extract a tar.gz or tar.zst archive from cache into a sub-directory of
    /// cache.  The top-level directory inside the archive is stripped so that
    /// the archive contents land directly in `dest_subdir`.
    ExtractArchive {
        archive_name: &'static str,
        dest_subdir: &'static str,
    },
    /// Copy compiled DLLs from an extracted directory into the Wine prefix
    /// system directories (system32 or syswow64).
    CopyDllsToPrefix {
        src_subdir: &'static str,
        dlls: &'static str,
        wine_dir: &'static str,
    },
    /// Run a winetricks verb inside the prefix using `umu-run winetricks`.
    RunWinetricks {
        verb: &'static str,
    },
    /// Remove DLLs from a Wine prefix system directory (reverses CopyDllsToPrefix).
    RemoveDllsFromPrefix {
        dlls: &'static str,
        wine_dir: &'static str,
    },
    /// Delete DLL override entries from the Wine registry (reverses OverrideDlls).
    RemoveDllOverrides {
        dlls: &'static str,
    },
}

#[derive(Clone)]
pub struct DepStep {
    pub description: &'static str,
    pub action: DepStepAction,
}

// ── Progress messages (sent from background thread → GTK main loop) ──────────

pub enum DepInstallMsg {
    Progress {
        step: usize,
        total: usize,
        description: String,
    },
    Done,
    Failed(String),
}

// ── Step execution engine ─────────────────────────────────────────────────────

pub fn execute_dep_step(
    step: &DepStep,
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
) -> Result<(), String> {
    match &step.action {
        DepStepAction::DownloadFile { url, file_name } => {
            if !url.starts_with("https://") {
                return Err(format!(
                    "Refusing to download '{}' from a non-HTTPS source",
                    file_name
                ));
            }
            let dest = format!("{}/{}", cache_dir, file_name);
            if std::path::Path::new(&dest).exists() {
                return Ok(());
            }
            let _ = fs::create_dir_all(cache_dir);
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
                    &dest,
                    url,
                ])
                .status()
                .map_err(|e| format!("curl unavailable: {}", e))?;
            if !status.success() {
                let _ = fs::remove_file(&dest);
                return Err(format!("Download failed for {}", file_name));
            }
            Ok(())
        }

        DepStepAction::RunExe {
            file_name,
            args,
            extra_env,
        } => {
            let exe_path = format!("{}/{}", cache_dir, file_name);
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            for pair in extra_env.split_whitespace() {
                if let Some(eq) = pair.find('=') {
                    cmd.env(&pair[..eq], &pair[eq + 1..]);
                }
            }
            let mut run_args = vec![exe_path];
            for arg in args.split_whitespace() {
                run_args.push(arg.to_string());
            }
            cmd.args(&run_args);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to launch {}: {}", file_name, e))?;
            if !status.success() {
                return Err(format!("Installer '{}' exited with an error", file_name));
            }
            Ok(())
        }

        DepStepAction::RunMsi { file_name, args } => {
            let msi_path = format!("{}/{}", cache_dir, file_name);
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            let mut run_args = vec!["msiexec.exe".to_string(), "/i".to_string(), msi_path];
            for arg in args.split_whitespace() {
                run_args.push(arg.to_string());
            }
            cmd.args(&run_args);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run msiexec for {}: {}", file_name, e))?;
            if !status.success() {
                return Err(format!("MSI install '{}' failed", file_name));
            }
            Ok(())
        }

        DepStepAction::OverrideDlls {
            dlls,
            override_type,
        } => {
            let reg_lines: Vec<String> = dlls
                .split(',')
                .map(|d| format!("\"{}\"=\"{}\"", d.trim(), override_type))
                .collect();
            let reg_content = format!(
                "Windows Registry Editor Version 5.00\r\n\r\n\
                 [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
                 {}\r\n",
                reg_lines.join("\r\n")
            );
            let safe_name = dlls
                .split(',')
                .next()
                .unwrap_or("dll")
                .trim()
                .replace(['-', '.'], "_");
            let reg_path = format!("{}/override_{}.reg", cache_dir, safe_name);
            fs::write(&reg_path, reg_content)
                .map_err(|e| format!("Failed to write .reg file: {}", e))?;

            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            cmd.args(["regedit.exe", "/S", &reg_path]);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run regedit: {}", e))?;
            let _ = fs::remove_file(&reg_path);
            if !status.success() {
                return Err(format!("DLL override registration failed for: {}", dlls));
            }
            Ok(())
        }

        DepStepAction::RegisterDll { dll } => {
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            cmd.args(["regsvr32.exe", "/s", dll]);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run regsvr32: {}", e))?;
            if !status.success() {
                return Err(format!("Failed to register DLL '{}'", dll));
            }
            Ok(())
        }

        DepStepAction::ExtractArchive {
            archive_name,
            dest_subdir,
        } => {
            let archive_path = format!("{}/{}", cache_dir, archive_name);
            let dest_path = format!("{}/{}", cache_dir, dest_subdir);
            fs::create_dir_all(&dest_path).map_err(|e| {
                format!(
                    "Failed to create extraction directory '{}': {}",
                    dest_path, e
                )
            })?;
            let mut cmd = std::process::Command::new("tar");
            if archive_name.ends_with(".tar.zst") || archive_name.ends_with(".tzst") {
                cmd.args([
                    "-I",
                    "zstd",
                    "-xf",
                    &archive_path,
                    "-C",
                    &dest_path,
                    "--strip-components=1",
                ]);
            } else {
                cmd.args([
                    "-xf",
                    &archive_path,
                    "-C",
                    &dest_path,
                    "--strip-components=1",
                ]);
            }
            let status = cmd
                .status()
                .map_err(|e| format!("tar unavailable: {}", e))?;
            if !status.success() {
                return Err(format!("Failed to extract '{}'", archive_name));
            }
            Ok(())
        }

        DepStepAction::CopyDllsToPrefix {
            src_subdir,
            dlls,
            wine_dir,
        } => {
            let src_dir = format!("{}/{}", cache_dir, src_subdir);
            let dst_dir = format!("{}/drive_c/windows/{}", prefix_path, wine_dir);
            fs::create_dir_all(&dst_dir)
                .map_err(|e| format!("Failed to create target directory '{}': {}", dst_dir, e))?;
            for dll in dlls.split(',') {
                let dll = dll.trim();
                let src = format!("{}/{}.dll", src_dir, dll);
                let dst = format!("{}/{}.dll", dst_dir, dll);
                fs::copy(&src, &dst).map_err(|e| format!("Failed to copy {}.dll: {}", dll, e))?;
            }
            Ok(())
        }

        DepStepAction::RunWinetricks { verb } => {
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            cmd.args(["winetricks", "-q", verb]);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run winetricks {}: {}", verb, e))?;
            if !status.success() {
                return Err(format!("winetricks '{}' failed", verb));
            }
            Ok(())
        }

        DepStepAction::RemoveDllsFromPrefix { dlls, wine_dir } => {
            let dst_dir = format!("{}/drive_c/windows/{}", prefix_path, wine_dir);
            for dll in dlls.split(',') {
                let dll = dll.trim();
                let path = format!("{}/{}.dll", dst_dir, dll);
                if let Err(e) = fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    leyen_log(
                        "WARN ",
                        &format!("Could not remove {}.dll from {}: {}", dll, wine_dir, e),
                    );
                }
            }
            Ok(())
        }

        DepStepAction::RemoveDllOverrides { dlls } => {
            let reg_lines: Vec<String> = dlls
                .split(',')
                .map(|d| format!("\"{}\"=-", d.trim()))
                .collect();
            let reg_content = format!(
                "Windows Registry Editor Version 5.00\r\n\r\n\
                 [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
                 {}\r\n",
                reg_lines.join("\r\n")
            );
            let safe_name = dlls
                .split(',')
                .next()
                .unwrap_or("dll")
                .trim()
                .replace(['-', '.'], "_");
            let reg_path = format!("{}/remove_override_{}.reg", cache_dir, safe_name);
            fs::write(&reg_path, reg_content)
                .map_err(|e| format!("Failed to write .reg file: {}", e))?;

            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
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
            cmd.args(["regedit.exe", "/S", &reg_path]);
            if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run regedit: {}", e))?;
            let _ = fs::remove_file(&reg_path);
            if !status.success() {
                return Err(format!("DLL override removal failed for: {}", dlls));
            }
            Ok(())
        }
    }
}

// ── Async orchestrator ────────────────────────────────────────────────────────

/// How often the GTK main loop polls the background-thread message queue.
pub const DEP_ASYNC_POLL_MS: u64 = 50;

pub fn install_dep_async(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
    on_progress: impl Fn(usize, usize, String) + 'static,
    on_finish: impl FnOnce(bool, Option<String>) + 'static,
) {
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        on_finish(false, Some("umu-launcher not ready".to_string()));
        return;
    }
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        on_finish(false, Some("umu-launcher not available".to_string()));
        return;
    }

    let steps = get_dep_steps(dep_id);
    if steps.is_empty() {
        let msg = format!("No install steps defined for '{}'", dep_id);
        overlay.add_toast(adw::Toast::new(&msg));
        on_finish(false, Some(msg));
        return;
    }

    let total = steps.len();
    let dep_id_t = dep_id.to_string();
    let prefix_t = prefix_path.to_string();
    let proton_t = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    let queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<DepInstallMsg>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let queue_bg = queue.clone();

    let on_finish = std::rc::Rc::new(std::cell::RefCell::new(Some(on_finish)));
    let on_progress = std::rc::Rc::new(on_progress);

    glib::timeout_add_local(
        std::time::Duration::from_millis(DEP_ASYNC_POLL_MS),
        move || {
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
                    DepInstallMsg::Done => {
                        if let Some(f) = on_finish.borrow_mut().take() {
                            f(true, None);
                        }
                        return glib::ControlFlow::Break;
                    }
                    DepInstallMsg::Failed(err) => {
                        if let Some(f) = on_finish.borrow_mut().take() {
                            f(false, Some(err));
                        }
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        },
    );

    std::thread::spawn(move || {
        leyen_log(
            "INFO ",
            &format!("[dep:{}] starting install ({} steps)", dep_id_t, total),
        );
        for (i, step) in steps.iter().enumerate() {
            leyen_log(
                "INFO ",
                &format!(
                    "[dep:{}] step {}/{}: {}",
                    dep_id_t,
                    i + 1,
                    total,
                    step.description
                ),
            );
            queue_bg.lock().unwrap().push_back(DepInstallMsg::Progress {
                step: i + 1,
                total,
                description: step.description.to_string(),
            });
            if let Err(e) = execute_dep_step(step, &prefix_t, &proton_t, &cache_dir) {
                leyen_log(
                    "ERROR",
                    &format!("[dep:{}] step {} failed: {}", dep_id_t, i + 1, e),
                );
                queue_bg.lock().unwrap().push_back(DepInstallMsg::Failed(e));
                return;
            }
        }
        leyen_log("INFO ", &format!("[dep:{}] install complete", dep_id_t));
        queue_bg.lock().unwrap().push_back(DepInstallMsg::Done);
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
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        on_finish(false, Some("umu-launcher not ready".to_string()));
        return;
    }
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        on_finish(false, Some("umu-launcher not available".to_string()));
        return;
    }

    let steps = get_dep_uninstall_steps(dep_id);
    if steps.is_empty() {
        on_finish(true, None);
        return;
    }

    let total = steps.len();
    let dep_id_t = dep_id.to_string();
    let prefix_t = prefix_path.to_string();
    let proton_t = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    let queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<DepInstallMsg>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let queue_bg = queue.clone();

    let on_finish = std::rc::Rc::new(std::cell::RefCell::new(Some(on_finish)));
    let on_progress = std::rc::Rc::new(on_progress);

    glib::timeout_add_local(
        std::time::Duration::from_millis(DEP_ASYNC_POLL_MS),
        move || {
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
                    DepInstallMsg::Done => {
                        if let Some(f) = on_finish.borrow_mut().take() {
                            f(true, None);
                        }
                        return glib::ControlFlow::Break;
                    }
                    DepInstallMsg::Failed(err) => {
                        if let Some(f) = on_finish.borrow_mut().take() {
                            f(false, Some(err));
                        }
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        },
    );

    std::thread::spawn(move || {
        leyen_log(
            "INFO ",
            &format!("[dep:{}] starting uninstall ({} steps)", dep_id_t, total),
        );
        for (i, step) in steps.iter().enumerate() {
            leyen_log(
                "INFO ",
                &format!(
                    "[dep:{}] uninstall step {}/{}: {}",
                    dep_id_t,
                    i + 1,
                    total,
                    step.description
                ),
            );
            queue_bg.lock().unwrap().push_back(DepInstallMsg::Progress {
                step: i + 1,
                total,
                description: step.description.to_string(),
            });
            if let Err(e) = execute_dep_step(step, &prefix_t, &proton_t, &cache_dir) {
                leyen_log(
                    "ERROR",
                    &format!("[dep:{}] uninstall step {} failed: {}", dep_id_t, i + 1, e),
                );
                queue_bg.lock().unwrap().push_back(DepInstallMsg::Failed(e));
                return;
            }
        }
        leyen_log("INFO ", &format!("[dep:{}] uninstall complete", dep_id_t));
        queue_bg.lock().unwrap().push_back(DepInstallMsg::Done);
    });
}
