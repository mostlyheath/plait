//! GPUI presentation state for the single selected thread workspace.
//!
//! Window-owned state (focus, approval key debounce, sidebar navigation,
//! command palette, settings pages, panels, and shared services) remains on
//! `Plait`. This container holds the thread-scoped presentation fields and
//! centralizes the existing reset paths so later stages can retain or park
//! workspaces without changing window ownership.

use super::{
    Workspace, changes, conversation_find, file_view, history, host, images, live_conversation,
    selection, text_field, thread_controls, thread_files, turn_records, turn_review,
};
use gpui::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Presentation container for the currently selected thread workspace.
pub(super) struct ThreadWorkspace {
    pub(super) diff_search: Option<(std::sync::Weak<plait_client::model::Diff>, String)>,
    pub(super) diff_find: file_view::Find,
    pub(super) work_counts: HashMap<std::path::PathBuf, String>,
    pub(super) work_area: Bounds<Pixels>,
    pub(super) restore: Option<plait_client::tabs::bookmark::ThreadBookmark>,
    pub(super) restore_pending: [bool; 4],
    pub(super) cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Existing per-thread editors; selection keeps their drafts and IME state.
    pub(super) composers: HashMap<String, Entity<super::composer::Composer>>,
    pub(super) host_watch: Option<host::WatchLease>,
    pub(super) live_scroll: ListState,
    pub(super) live_count: usize,
    pub(super) live_layout: plait_client::folding::Layout,
    /// Earlier turns opened in the active tab (mirror of `TabSession::opened_turns`).
    pub(super) opened_turns: HashSet<String>,
    pub(super) formatted: Arc<plait_client::markdown::Transcript>,
    pub(super) history: history::HistoryView,
    pub(super) images: images::ImageCache,
    /// Diff lines picked in Changes to ask the selected thread about.
    pub(super) line_pick: Option<changes::LinePick>,
    /// Files the selected thread changed, above its composer.
    pub(super) thread_files: thread_files::ThreadFiles,
    pub(super) turn_records: turn_records::TurnRecords,
    pub(super) turn_review: turn_review::TurnReview,
    pub(super) reveal: live_conversation::Reveals,
    /// The inspector toggle was used on a window too narrow to show it.
    pub(super) thread_controls: thread_controls::ThreadControls,
    pub(super) expanded_live_tools: HashSet<String>,
    pub(super) reading: selection::Reading,
    pub(super) file_scroll: UniformListScrollHandle,
    pub(super) diff_scroll: UniformListScrollHandle,
    /// The History view's commit list and the selected commit's patch.
    pub(super) history_scroll: UniformListScrollHandle,
    pub(super) commit_scroll: UniformListScrollHandle,
    /// The commit message in Changes. Kept across view switches until it commits.
    pub(super) commit_message: Entity<text_field::TextField>,
    pub(super) tab: Workspace,
    pub(super) file_viewer: file_view::FileViewer,
    pub(super) conversation_find: conversation_find::ConversationFind,
}

impl ThreadWorkspace {
    pub(super) fn new(commit_message: Entity<text_field::TextField>) -> Self {
        Self {
            diff_search: None,
            diff_find: Default::default(),
            work_counts: Default::default(),
            work_area: Default::default(),
            restore: None,
            restore_pending: [false; 4],
            cancelled: Arc::default(),
            composers: HashMap::new(),
            host_watch: None,
            live_scroll: ListState::new(0, ListAlignment::Bottom, px(300.)),
            live_count: 0,
            live_layout: Default::default(),
            opened_turns: HashSet::new(),
            formatted: Default::default(),
            history: Default::default(),
            images: Default::default(),
            line_pick: None,
            thread_files: Default::default(),
            turn_records: Default::default(),
            turn_review: Default::default(),
            reveal: Default::default(),
            thread_controls: Default::default(),
            expanded_live_tools: HashSet::new(),
            reading: Default::default(),
            file_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            history_scroll: UniformListScrollHandle::new(),
            commit_scroll: UniformListScrollHandle::new(),
            commit_message,
            tab: Workspace::Conversation,
            file_viewer: Default::default(),
            conversation_find: Default::default(),
        }
    }

    /// Clear active Recorded Changes state while keeping host capability cache.
    pub(super) fn clear_turn_review(&mut self) {
        let state = &mut self.turn_review;
        state.generation += 1;
        state.target = None;
        state.window = None;
        state.record = None;
        state.diff = None;
        state.status = None;
        state.selected = None;
        state.current = false;
        state.error = None;
        state.loading = false;
        state.pending = false;
    }

    /// Reset connection-bound presentation state when reconnecting the host.
    pub(super) fn reset_for_reconnect(&mut self) {
        self.host_watch = None;
        self.images = Default::default();
        self.history = Default::default();
        self.turn_review.generation += 1;
    }

    /// Preserve today's reset path when selecting a different host thread.
    pub(super) fn reset_for_thread_selection(&mut self) {
        self.host_watch = None;
        self.images = Default::default();
        self.clear_turn_review();
        self.history = Default::default();
        self.opened_turns.clear();
        self.tab = Workspace::Conversation;
        self.formatted = Default::default();
    }

    /// Reset checkout-bound presentation handles when switching checkouts.
    pub(super) fn reset_for_checkout_change(&mut self) {
        self.history = Default::default();
        self.host_watch = None;
        self.file_scroll = UniformListScrollHandle::new();
    }
}
