use std::path::PathBuf;

pub use plait_protocol::model::*;

/// An independent execution environment. Before M3 only this computer exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    pub local: bool,
}

/// A checkout and the machines that hold it. Threads come from each machine's host.
pub struct Project {
    pub checkout: PathBuf,
    pub name: String,
    pub machines: Vec<Machine>,
}
