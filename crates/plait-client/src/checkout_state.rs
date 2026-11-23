//! Checkout-scoped plain Rust state: shared repository data, write jobs, and
//! the legacy direct-creation selection. Independent drafts own their requests.

use crate::{
    git,
    host::{ModelChoice, ThinkingLevel},
    model::{Diff, Snapshot},
    recent::Recent,
    state::{DIFF_CACHE, Job, Load, WriteState},
    thread_view::DiffKey,
};
use std::{path::PathBuf, sync::Arc};

/// Direct-creation selection: active checkout path, chosen model,
/// and any unconfirmed creation request waiting for acknowledgement or retry.
pub struct CheckoutCreationState {
    pub checkout: PathBuf,
    /// Changes on checkout selection, independently of the host connection.
    pub generation: u64,
    pub new_model: Option<ModelChoice>,
    pub pending_create: Option<(String, ModelChoice)>,
    /// The starting thinking level fixed when `pending_create` was made, so a
    /// retry repeats the same request even if the default changes meanwhile.
    pub pending_thinking: Option<ThinkingLevel>,
    pub creating: bool,
}

impl CheckoutCreationState {
    pub fn new(checkout: PathBuf) -> Self {
        Self {
            checkout,
            generation: 0,
            new_model: None,
            pending_create: None,
            pending_thinking: None,
            creating: false,
        }
    }

    /// Clear transient in-flight creation flag on reconnect while keeping any
    /// unconfirmed creation request ID and model for safe retry.
    pub fn disconnected(&mut self) {
        self.creating = false;
    }
}

/// Shared commit-list data and immutable patch cache for a checkout.
#[derive(Default)]
pub struct CommitLogData {
    pub list: Load<git::History>,
    /// A further page was asked for and has not arrived.
    pub loading_more: bool,
    /// Guards history replies. Separate from status, so staging keeps history.
    pub(crate) epoch: u64,
    pub(crate) queued_page: Option<usize>,
    /// Patches by commit ID. A commit never changes, so these survive reloads.
    pub(crate) shows: Recent<String, Arc<Diff>>,
}

/// Shared checkout repository data, caches, and background job scheduling.
pub struct CheckoutRepositoryData {
    pub path: PathBuf,
    pub snapshot: Load<Arc<Snapshot>>,
    /// Loaded on request and kept until the next refresh or checkout change.
    pub files: Load<Arc<git::FileList>>,
    pub(crate) epoch: u64,
    pub(crate) running: bool,
    pub(crate) pending: Option<Job>,
    // Separate from `pending`, so a file list and a diff cannot displace each other.
    pub(crate) files_queued: bool,
    /// Recently read diffs, so moving between files does not rerun Git.
    pub(crate) cache: Recent<DiffKey, Arc<Diff>>,
    /// The next file's diff, read ahead while nothing else waits.
    pub(crate) prefetch: Option<Job>,
    /// Staging and committing; see `RepositoryState::request_write`.
    pub write: WriteState,
    /// Count behind `RepositoryState::staged()`; set when a status arrives.
    pub(crate) staged: usize,
    /// Commit history pages and immutable patch cache.
    pub commits: CommitLogData,
    /// Count behind `RepositoryState::conflicts()`; set when a status arrives.
    pub(crate) conflicts: usize,
    /// The repository root from the last status read. Kept while a refresh
    /// runs, so writes and history need not wait for it; cleared on `open`.
    pub(crate) root: Option<PathBuf>,
    /// Local branches, read each time the branch picker opens.
    pub branches: Load<Arc<git::Branches>>,
    /// Counts branch requests, so only the newest reply is kept.
    pub(crate) branch_request: u64,
    pub(crate) branches_queued: bool,
}

impl CheckoutRepositoryData {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            snapshot: Load::Empty,
            files: Load::Empty,
            epoch: 0,
            running: false,
            pending: None,
            files_queued: false,
            cache: Recent::new(DIFF_CACHE),
            prefetch: None,
            write: WriteState::default(),
            staged: 0,
            commits: CommitLogData::default(),
            conflicts: 0,
            root: None,
            branches: Load::Empty,
            branch_request: 0,
            branches_queued: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_creation_defaults_and_disconnect_preserve_pending_id() {
        let mut creation = CheckoutCreationState::new(PathBuf::from("/repo"));
        assert_eq!(creation.checkout, PathBuf::from("/repo"));
        assert!(creation.new_model.is_none());
        assert!(creation.pending_create.is_none());
        assert!(creation.pending_thinking.is_none());
        assert!(!creation.creating);

        creation.creating = true;
        creation.pending_create = Some((
            "req-1".into(),
            ModelChoice {
                provider: "p".into(),
                model_id: "m".into(),
            },
        ));
        creation.disconnected();
        assert!(!creation.creating);
        assert_eq!(creation.pending_create.as_ref().unwrap().0, "req-1");
    }
}
