//! Host release installation.
pub mod install;
#[cfg(target_os = "linux")]
pub mod linux;
pub use plait_release::{HostRelease, check, download, host_release, sha256_file};

pub use plait_protocol::update::*;
use plait_release::curl;
fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid(detail.into())
}
