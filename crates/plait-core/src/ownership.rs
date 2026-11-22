//! Stable operation owners and short-lived read contexts. No UI state lives here.
pub use plait_engine::ownership::CheckoutKeyExt;
pub use plait_protocol::ownership::{CheckoutKey, HostKey, ThreadKey};
use std::path::PathBuf;

pub trait HostKeyExt {
    fn local() -> Option<Self>
    where
        Self: Sized;
}
impl HostKeyExt for HostKey {
    fn local() -> Option<Self> {
        let store = std::env::var_os("PLAIT_HOST_STORE")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/plait"))
            })?;
        store.is_absolute().then_some(Self(store))
    }
}

/// Selection and checkout lifetimes advance independently of the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadStamp {
    pub connection: u64,
    pub selection: u64,
    pub checkout: u64,
    pub thread: Option<ThreadKey>,
}

/// Acknowledgements use this owner, never a visible selection generation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OperationKey {
    pub thread: ThreadKey,
    pub request: String,
}
