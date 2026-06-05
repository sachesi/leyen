pub mod proton;
pub mod umu;

pub use proton::{check_or_install_protonge, detect_proton_versions};
pub use umu::{check_or_install_umu, check_or_install_winetricks};
