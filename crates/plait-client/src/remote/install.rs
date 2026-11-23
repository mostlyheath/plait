//! Remote host installation information.

/// What this Mac learned about the machine before installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The platform a host archive is published for, such as `linux-x86_64`.
    pub platform: Option<String>,
    /// `uname -sm`, to name an unsupported machine.
    pub system: String,
    /// A host already answers `plait-host version` there.
    pub installed: bool,
    pub downloader: bool,
}

#[cfg(feature = "ssh")]
#[path = "install_process.rs"]
mod process;
#[cfg(feature = "ssh")]
pub use process::*;
