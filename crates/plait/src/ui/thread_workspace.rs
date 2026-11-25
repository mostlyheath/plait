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
    pub(super) panel_change_total: Option<String>,
    pub(super) panel_file_counts: HashMap<std::path::PathBuf, String>,
    pub(super) sheet_body: Bounds<Pixels>,
    pub(super) restore: Option<plait_client::tabs::bookmark::ThreadBookmark>,
    pub(super) restore_pending: [bool; 4],
    /// A restore notice belongs to the patch whose saved selection no longer fits.
    pub(super) changed_patch: std::sync::Weak<plait_client::model::Diff>,
    pub(super) cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Existing per-thread editors; selection keeps their drafts and IME state.
    pub(super) composers: HashMap<String, Entity<super::composer::Composer>>,
    /// Where each composer saves, set as it mounts (#132).
    pub(super) bindings: HashMap<String, super::saved_work::Binding>,
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
    /// Pending approval ID for which `Reply instead` draws the message box
    /// below the approval card. Device-ephemeral and cleared when answered.
    approval_reply_id: Option<String>,
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
            panel_file_counts: Default::default(),
            panel_change_total: None,
            sheet_body: Default::default(),
            restore: None,
            restore_pending: [false; 4],
            changed_patch: Default::default(),
            cancelled: Arc::default(),
            composers: HashMap::new(),
            bindings: HashMap::new(),
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
            approval_reply_id: None,
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

    /// Whether `Reply instead` has revealed the message box for `approval_id`.
    pub(super) fn showing_composer_for_approval(&self, approval_id: &str) -> bool {
        self.approval_reply_id.as_deref() == Some(approval_id)
    }

    /// Show the message box below `approval_id` until it is answered or replaced.
    pub(super) fn show_composer_for_approval(&mut self, approval_id: String) {
        self.approval_reply_id = Some(approval_id);
    }

    /// Hide the extra message box once the approval is answered.
    pub(super) fn clear_approval_reply(&mut self) {
        self.approval_reply_id = None;
    }

    /// Reset connection-bound presentation state when reconnecting the host.
    pub(super) fn reset_for_reconnect(&mut self) {
        self.host_watch = None;
        self.images = Default::default();
        self.history = Default::default();
        self.turn_review.generation += 1;
        self.approval_reply_id = None;
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
        self.approval_reply_id = None;
    }

    /// Reset checkout-bound presentation handles when switching checkouts.
    pub(super) fn reset_for_checkout_change(&mut self) {
        self.history = Default::default();
        self.host_watch = None;
        self.file_scroll = UniformListScrollHandle::new();
    }
}
