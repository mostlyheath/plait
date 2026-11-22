use std::path::PathBuf;

/// The local host endpoint is this protocol's identity; it has no server UUID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostKey(pub PathBuf);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThreadKey {
    pub host: Option<HostKey>,
    pub id: String,
}

/// A root pathname alone does not identify a checkout after a directory swap.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CheckoutKey {
    pub host: Option<HostKey>,
    pub root: PathBuf,
    pub device: u64,
    pub inode: u64,
}
