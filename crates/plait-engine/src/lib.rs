//! Host-side repository operations and storage.

pub mod build_output;
pub mod file_view;
pub mod git;
pub mod lease;
pub mod worktree_size;

pub use plait_protocol::{display_text, model, patch, wire};
