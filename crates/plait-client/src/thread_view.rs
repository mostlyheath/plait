//! Presentation-independent thread workspace and view selection state.
//!
//! These plain Rust types separate what a thread's views are looking at
//! (selected diff, selected commit, active view, and selected-thread host read
//! state) from shared checkout data, host connection state, and GPUI handles.

use crate::{
    host::Snapshot,
    model::{Diff, DiffMode},
    state::Load,
};
use std::{collections::HashSet, path::PathBuf, sync::Arc};

/// The four workspace views a thread can display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadView {
    #[default]
    Conversation,
    Changes,
    /// Commit history and each commit's patch.
    History,
    /// One checkout file, read-only.
    Files,
}

/// Identifies a changed file and diff layer within a status snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffKey {
    pub file: usize,
    pub path: PathBuf,
    pub mode: DiffMode,
}

/// Per-view selection and loaded diff for the Changes view.
#[derive(Debug, Default)]
pub struct ChangesViewState {
    pub selected: Option<DiffKey>,
    pub diff: Load<Arc<Diff>>,
    pub(crate) restore: Option<(PathBuf, DiffMode)>,
    /// Set while stepping through files, with the direction. A click is
    /// random access, so it reads nothing ahead that it might wait behind.
    pub(crate) ahead: Option<bool>,
}

/// Per-view selection and loaded patch for the Git History view.
#[derive(Debug, Default)]
pub struct HistoryViewState {
    pub selected: Option<String>,
    pub patch: Load<Arc<Diff>>,
    /// The History view is showing; a commit reloads the list only then.
    pub(crate) wanted: bool,
    pub(crate) queued_show: Option<String>,
}

/// Thread-scoped Git view state, separate from shared checkout data and writes.
#[derive(Debug, Default)]
pub struct ThreadRepositoryView {
    pub changes: ChangesViewState,
    pub history: HistoryViewState,
}

/// Host read and control state for the currently selected thread slot.
#[derive(Default)]
pub struct SelectedThreadState {
    pub watch_epoch: u64,
    pub selected: Option<String>,
    pub snapshot: Option<Arc<Snapshot>>,
    pub live_preview_clipped: bool,
    pub submitting: bool,
    pub controlling: bool,
    pub decided_approvals: HashSet<String>,
}

impl SelectedThreadState {
    /// Select a different thread (or none), advancing the watch epoch and
    /// clearing the previous thread's snapshot and control-busy state.
    pub fn select(&mut self, id: Option<String>) -> u64 {
        self.watch_epoch += 1;
        self.selected = id;
        self.snapshot = None;
        self.live_preview_clipped = false;
        self.controlling = false;
        self.watch_epoch
    }

    /// Clear transient connection-bound read/control state on reconnect.
    pub fn disconnected(&mut self) {
        self.watch_epoch += 1;
        self.snapshot = None;
        self.live_preview_clipped = false;
        self.submitting = false;
        self.controlling = false;
        self.decided_approvals.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_initial_selected_thread_and_view_state() {
        assert_eq!(ThreadView::default(), ThreadView::Conversation);
        let view = ThreadRepositoryView::default();
        assert!(view.changes.selected.is_none());
        assert!(matches!(view.changes.diff, Load::Empty));
        assert!(view.changes.ahead.is_none());
        assert!(view.history.selected.is_none());
        assert!(matches!(view.history.patch, Load::Empty));
        assert!(!view.history.wanted);
        assert!(view.history.queued_show.is_none());

        let selected = SelectedThreadState::default();
        assert_eq!(selected.watch_epoch, 0);
        assert!(selected.selected.is_none());
        assert!(selected.snapshot.is_none());
        assert!(!selected.submitting);
        assert!(!selected.controlling);
        assert!(selected.decided_approvals.is_empty());
    }
}
