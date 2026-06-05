pub mod proton;
pub mod umu;

pub use leyen_model::runtime::detect_proton_versions;
pub use proton::check_or_install_protonge;
pub use umu::{check_or_install_umu, check_or_install_winetricks};
