//! Headless models, client state, local host transport, and read-only Git access.
//!
//! Blocking Git and host operations must run on the caller's background executor.

pub mod build_output;
pub mod checkout_state;
pub mod compose;
pub mod conversation_find;
pub mod diff;
pub mod file_view;
pub mod fuzzy;
pub mod git;
pub mod handoff;
pub mod highlight;
pub mod host;
pub mod host_state;
pub mod images;
pub mod lease;
pub mod local_socket;
pub mod machine;
pub mod markdown;
pub mod model;
pub mod operations;
pub mod ownership;
pub mod patch;
pub mod preferences;
pub mod recent;
pub mod reveal;
pub mod review;
pub mod selection;
pub mod session;
pub mod state;
pub mod tabs;
pub mod theme;
pub mod thread_view;
pub mod toasts;
pub mod tool_view;
pub mod transcript;
pub mod update;

pub mod history;
pub mod navigation;

pub mod display_text;
pub mod worktree_graph;
pub mod worktree_removal;
pub mod worktree_size;
