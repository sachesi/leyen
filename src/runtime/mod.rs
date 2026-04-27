pub mod proton;
pub mod umu;

pub use proton::{check_or_install_protonge, detect_proton_versions, resolve_proton_path};
pub use umu::{
    check_for_umu_updates, check_or_install_umu, get_local_umu_run_path, get_local_umu_version,
    get_umu_core_dir, get_umu_run_path, get_umu_runtime_dir, is_umu_run_available,
};
