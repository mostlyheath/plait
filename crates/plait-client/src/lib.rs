//! Headless models, client state, local host transport, and read-only Git access.
//!
//! Blocking Git and host operations must run on the caller's background executor.

pub mod automations;
pub use plait_protocol::build_output;
pub mod checkout_state;
pub mod compose;
pub mod connections;
pub mod conversation_find;
pub mod diff;
pub mod doc_links;
pub mod file_view;
pub mod folding;
pub mod fuzzy;
pub use plait_protocol::git;
pub mod git_step;
pub mod graph_view;
pub mod handoff;
pub mod highlight;
pub mod host;
pub mod host_state;
pub mod images;
pub use plait_protocol::lease;
pub mod linear_drafts;
pub mod linear_issues;
pub mod linear_settings;
#[cfg(feature = "local-host")]
pub mod local_socket;
pub mod machine;
pub mod machines;
pub mod markdown;
pub mod model;
pub mod operations;
pub mod ownership;
pub mod panel;
pub mod panes;
pub mod patch;
pub mod preferences;
pub mod projects;
pub mod recent;
pub mod remote;
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
pub mod wire;

pub mod history;
pub mod navigation;

pub mod display_text;
pub mod worktree_graph;
pub mod worktree_removal;
pub use plait_protocol::worktree_size;
