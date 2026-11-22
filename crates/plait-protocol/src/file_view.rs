use std::time::SystemTime;

/// Larger files are described, not drawn.
pub const READ_LIMIT: u64 = 8 * 1024 * 1024;

/// Size and modification time: enough to skip rereading an unchanged file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub len: u64,
    pub device: u64,
    pub inode: u64,
    pub modified: Option<SystemTime>,
}
