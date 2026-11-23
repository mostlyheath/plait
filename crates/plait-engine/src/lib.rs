//! Host-side repository operations and storage.

pub mod build_output;
pub mod file_view;
pub use plait_transport::frame;
pub mod git;
pub mod host_git;
pub mod lease;
pub use plait_transport::local_socket;
pub mod ownership;
pub mod worktree_size;

pub use plait_protocol::{display_text, model, patch, wire};

pub mod update;
