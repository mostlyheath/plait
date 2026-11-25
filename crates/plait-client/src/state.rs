mod backend;
mod registry;
pub use backend::Backend;
pub use registry::{CheckoutId, CheckoutRegistry, RepositoryJob, RepositoryReply};

pub use crate::{
    checkout_state::{CheckoutRepositoryData, CommitLogData},
    thread_view::{ChangesViewState, DiffKey, HistoryViewState, ThreadRepositoryView},
};
use crate::{
    git,
    model::{Diff, DiffMode, Project, Snapshot},
};
#[cfg(feature = "local-host")]
use crate::{machine, model::Machine};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Default)]
pub enum Load<T> {
    #[default]
    Empty,
    Loading,
    Ready(T),
    Error(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        if let Self::Ready(value) = self {
            Some(value)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub enum Job {
    Status {
        epoch: u64,
        path: PathBuf,
    },
    Diff {
        epoch: u64,
        key: DiffKey,
        snapshot: Arc<Snapshot>,
    },
    /// The checkout's file list, for naming files in a prompt.
    Files {
        epoch: u64,
        root: PathBuf,
    },
    /// A write the user asked for. Not tied to an epoch: it runs even if the
    /// view refreshes meanwhile, and its result is reported against `root`.
    Write {
        root: PathBuf,
        op: git::WriteOp,
    },
    /// A page of commit history: `skip` commits back from `tip`, or from
    /// HEAD for the first page. Later pages count from the first page's
    /// newest commit, so a commit landing meanwhile cannot shift them.
    Log {
        epoch: u64,
        root: PathBuf,
        tip: Option<String>,
        skip: usize,
    },
    /// One commit's message and patch.
    Show {
        epoch: u64,
        root: PathBuf,
        id: String,
    },
    /// Local branches, for the branch picker. `request` guards the reply.
    Branches {
        request: u64,
        root: PathBuf,
    },
}

pub enum Reply {
    Status(Result<Arc<Snapshot>, String>),
    Diff(Result<Arc<Diff>, String>),
    Files(Result<Arc<git::FileList>, String>),
    Write(Result<git::WriteOutcome, String>),
    Log(Result<git::History, String>),
    Show(Result<Arc<Diff>, String>),
    Branches(Result<Arc<git::Branches>, String>),
}

/// Diffs kept for quick switching. A diff is at most 10,000 parsed lines, so
/// this bounds memory to a few tens of megabytes in the worst case.
pub(crate) const DIFF_CACHE: usize = 16;

/// Commits listed per history page.
pub const HISTORY_PAGE: usize = 100;

/// Coordinates shared checkout repository data (`data`) with the single
/// selected workspace slot's Git view selection (`view`).
pub struct RepositoryState {
    /// Shared checkout status, file list, branches, caches, writes, and job queue.
    pub data: CheckoutRepositoryData,
    /// Selected-thread Git view state: selected changed file/diff and commit/patch.
    pub view: ThreadRepositoryView,
    pub(crate) view_generation: u64,
}

/// The one write that may be queued or running, and how the last one went.
#[derive(Default)]
pub struct WriteState {
    queued: Option<git::WriteOp>,
    running: bool,
    /// The last result in words, and whether it failed. Cleared by the next write.
    pub note: Option<(String, bool)>,
    /// The file to select again once the status after a write arrives.
    reselect: Option<(PathBuf, DiffMode)>,
    /// Set when a commit succeeds, so the view can clear its message once.
    pub committed: Option<String>,
    /// The last discard's saved copy, offered as Undo until the next write.
    pub undo: Option<git::Backup>,
}

impl WriteState {
    /// A write is waiting or running, so controls that start one are disabled.
    pub fn busy(&self) -> bool {
        self.queued.is_some() || self.running
    }
}

impl RepositoryState {
    fn bound_payload_cache(&mut self) {
        const LIMIT: usize = 16 * 1024 * 1024;
        self.data
            .cache
            .trim_weight(LIMIT, |diff| diff.retained_bytes());
        let remaining = LIMIT.saturating_sub(self.data.cache.weight(|diff| diff.retained_bytes()));
        self.data
            .commits
            .shows
            .trim_weight(remaining, |diff| diff.retained_bytes());
    }
    pub fn new(path: PathBuf) -> Self {
        let mut state = Self {
            data: CheckoutRepositoryData::new(path),
            view: ThreadRepositoryView::default(),
            view_generation: 0,
        };
        state.refresh();
        state
    }

    /// Retire a view's outstanding replies without invalidating shared data.
    pub fn invalidate_view(&mut self) {
        self.view_generation += 1;
        self.data.write.reselect = None;
        // Retiring a reply must not leave the current slot loading forever.
        if matches!(self.view.changes.diff, Load::Loading)
            && let Some(key) = self.view.changes.selected.clone()
        {
            self.show(key.file, key.mode);
        }
        if matches!(self.view.history.patch, Load::Loading) {
            self.view.history.queued_show = self.view.history.selected.clone();
        }
    }

    fn reset_view(&mut self) {
        let wanted = self.view.history.wanted;
        self.view = ThreadRepositoryView::default();
        self.view.history.wanted = wanted;
        self.invalidate_view();
    }

    pub fn path(&self) -> &Path {
        &self.data.path
    }

    pub fn snapshot(&self) -> &Load<Arc<Snapshot>> {
        &self.data.snapshot
    }

    pub fn files(&self) -> &Load<Arc<git::FileList>> {
        &self.data.files
    }

    pub fn branches(&self) -> &Load<Arc<git::Branches>> {
        &self.data.branches
    }

    pub fn write(&self) -> &WriteState {
        &self.data.write
    }

    pub fn write_mut(&mut self) -> &mut WriteState {
        &mut self.data.write
    }

    pub fn selected_diff(&self) -> Option<&DiffKey> {
        self.view.changes.selected.as_ref()
    }

    pub fn diff(&self) -> &Load<Arc<Diff>> {
        &self.view.changes.diff
    }

    pub fn commit_list(&self) -> &Load<git::History> {
        &self.data.commits.list
    }

    pub fn commit_loading_more(&self) -> bool {
        self.data.commits.loading_more
    }

    pub fn selected_commit(&self) -> Option<&str> {
        self.view.history.selected.as_deref()
    }

    pub fn commit_patch(&self) -> &Load<Arc<Diff>> {
        &self.view.history.patch
    }

    /// Switch checkouts. Nothing from the previous one carries over.
    pub fn open(&mut self, path: PathBuf) {
        self.view.changes = ChangesViewState::default();
        self.data.path = path;
        self.data.root = None;
        // A list of the previous checkout's branches must not land.
        self.data.branch_request += 1;
        self.data.branches = Load::Empty;
        self.data.branches_queued = false;
        let wanted = self.view.history.wanted;
        self.data.commits = CommitLogData {
            epoch: self.data.commits.epoch + 1,
            ..CommitLogData::default()
        };
        self.view.history = HistoryViewState {
            wanted,
            ..HistoryViewState::default()
        };
        self.refresh();
    }

    /// Reread everything: status, diffs, the file list, and shown history.
    pub fn refresh(&mut self) {
        self.refresh_status();
        self.reset_history();
    }

    /// Retire presentation reads; an accepted write still completes for its owner.
    pub fn park_view(&mut self) {
        self.view_generation += 1;
        self.view = ThreadRepositoryView::default();
        self.data.pending = self
            .data
            .pending
            .take()
            .filter(|job| matches!(job, Job::Status { .. }));
        self.data.prefetch = None;
        self.data.files_queued = false;
        self.data.files = Load::Empty;
        self.data.cache.clear();
        self.data.commits = CommitLogData {
            epoch: self.data.commits.epoch + 1,
            ..Default::default()
        };
    }

    pub fn restore_path(&mut self, path: PathBuf, mode: DiffMode) {
        if let Some(index) = self
            .snapshot()
            .ready()
            .and_then(|s| s.changes.iter().position(|c| c.path == path))
        {
            self.select(index, mode);
        } else {
            self.view.changes.restore = Some((path, mode));
        }
    }

    /// Reread status and diffs only. Staging changes the index, not history,
    /// so it keeps the commit list and its open patch.
    fn refresh_status(&mut self) {
        self.view.changes.restore = self
            .selected_change()
            .or_else(|| self.view.changes.restore.take());
        self.data.epoch += 1;
        self.data.snapshot = Load::Loading;
        self.data.staged = 0;
        self.data.conflicts = 0;
        self.view.changes.diff = Load::Empty;
        self.view.changes.selected = None;
        self.data.files = Load::Empty;
        self.data.files_queued = false;
        self.data.cache.clear();
        self.data.prefetch = None;
        self.data.pending = Some(Job::Status {
            epoch: self.data.epoch,
            path: self.data.path.clone(),
        });
    }

    /// Reread the commit list if the History view is showing, or drop it to
    /// load on the next visit. The selected commit and cached patches stay:
    /// commits are immutable, so only the list can be out of date.
    fn reset_history(&mut self) {
        self.data.commits.epoch += 1;
        self.data.commits.list = Load::Empty;
        self.data.commits.loading_more = false;
        self.data.commits.queued_page = None;
        // A patch still loading was guarded by the old epoch; ask again.
        if let (Some(id), Load::Loading) = (&self.view.history.selected, &self.view.history.patch) {
            self.view.history.queued_show = Some(id.clone());
        }
        if self.view.history.wanted {
            self.request_history();
        }
    }

    /// Changed files with something staged, counted once per status read.
    pub fn staged(&self) -> usize {
        self.data.staged
    }

    /// Changed files Git reports as unmerged, counted once per status read.
    pub fn conflicts(&self) -> usize {
        self.data.conflicts
    }

    /// Show a file's diff, as a click does.
    pub fn select(&mut self, file: usize, mode: DiffMode) {
        self.view.changes.ahead = None;
        self.show(file, mode);
    }

    /// Show a file's diff while stepping through the list, and read the one
    /// after it (in the same direction) ahead of time.
    pub fn step(&mut self, file: usize, mode: DiffMode, forward: bool) {
        self.view.changes.ahead = Some(forward);
        self.show(file, mode);
    }

    fn show(&mut self, file: usize, mode: DiffMode) {
        let Some(snapshot) = self.data.snapshot.ready() else {
            return;
        };
        if file >= snapshot.changes.len() {
            return;
        }
        let key = DiffKey {
            file,
            path: snapshot.changes[file].path.clone(),
            mode,
        };
        self.view.changes.selected = Some(key.clone());
        if let Some(diff) = self.data.cache.get(&key).cloned() {
            self.view.changes.diff = Load::Ready(diff);
            self.data.pending = None;
            self.queue_prefetch(&key);
        } else {
            self.view.changes.diff = Load::Loading;
            self.data.pending = Some(Job::Diff {
                epoch: self.data.epoch,
                key,
                snapshot: snapshot.clone(),
            });
        }
    }

    /// While stepping, read the next file in that direction ahead of time, in
    /// the view it would open in, so the next step shows a cached diff. One
    /// file only: one background Git process per step.
    fn queue_prefetch(&mut self, from: &DiffKey) {
        self.data.prefetch = None;
        let (Some(forward), Some(snapshot)) = (self.view.changes.ahead, self.data.snapshot.ready())
        else {
            return;
        };
        let count = snapshot.changes.len();
        // Stepping wraps around the list, so read-ahead does too.
        let file = if forward {
            (from.file + 1) % count.max(1)
        } else {
            (from.file + count - 1) % count.max(1)
        };
        if file == from.file {
            return;
        }
        let Some(change) = snapshot.changes.get(file) else {
            return;
        };
        let mode = if change.unstaged() {
            DiffMode::Unstaged
        } else {
            DiffMode::Staged
        };
        let key = DiffKey {
            file,
            path: change.path.clone(),
            mode,
        };
        if !self.data.cache.contains(&key) {
            self.data.prefetch = Some(Job::Diff {
                epoch: self.data.epoch,
                key,
                snapshot: snapshot.clone(),
            });
        }
    }

    /// Changes on every refresh and checkout change; guards cached Git data.
    pub fn epoch(&self) -> u64 {
        self.data.epoch
    }

    /// Ask for the checkout's file list. A cached or already queued list is kept.
    pub fn request_files(&mut self) {
        if matches!(self.data.files, Load::Loading | Load::Ready(_)) {
            return;
        }
        if let Load::Error(error) = &self.data.snapshot {
            self.data.files = Load::Error(error.clone());
            return;
        }
        self.data.files = Load::Loading;
        self.data.files_queued = true;
    }

    /// Status and diffs go first. The list needs the root a status read reports.
    fn files_job(&mut self) -> Option<Job> {
        if !self.data.files_queued {
            return None;
        }
        let root = self.data.snapshot.ready()?.root.clone();
        self.data.files_queued = false;
        Some(Job::Files {
            epoch: self.data.epoch,
            root,
        })
    }

    /// Queue a write. Refused while another is queued or running, or before a
    /// status has found the repository root. Returns whether it was queued.
    pub fn request_write(&mut self, op: git::WriteOp) -> bool {
        if self.data.write.busy() || self.data.root.is_none() {
            return false;
        }
        // Keep the file in view across the refresh the write causes.
        self.data.write.reselect = self.selected_change();
        self.data.write.note = None;
        self.data.write.undo = None;
        self.data.write.queued = Some(op);
        true
    }

    /// The selected file's path and view, if one is selected.
    fn selected_change(&self) -> Option<(PathBuf, DiffMode)> {
        let key = self.view.changes.selected.as_ref()?;
        Some((key.path.clone(), key.mode))
    }

    fn write_job(&mut self) -> Option<Job> {
        // `root` survives a refresh, so a write never waits on status. Only
        // `open` clears it; a write queued for the old checkout is then
        // refused with a note rather than run against the new one.
        let root = self.data.root.clone();
        let op = self.data.write.queued.take()?;
        let Some(root) = root else {
            self.data.write.note = Some((
                "Plait lost track of the repository; try again.".into(),
                true,
            ));
            return None;
        };
        self.data.write.running = true;
        Some(Job::Write { root, op })
    }

    /// Read the local branches again, for the branch picker. Branches change
    /// outside Plait, so an open picker never shows an older list.
    pub fn request_branches(&mut self) {
        self.data.branch_request += 1;
        if self.data.root.is_none()
            && let Load::Error(error) = &self.data.snapshot
        {
            self.data.branches = Load::Error(error.clone());
            self.data.branches_queued = false;
            return;
        }
        self.data.branches = Load::Loading;
        self.data.branches_queued = true;
    }

    /// Branches need the root a status read reports.
    fn branches_job(&mut self) -> Option<Job> {
        if !self.data.branches_queued {
            return None;
        }
        let root = self.data.root.clone()?;
        self.data.branches_queued = false;
        Some(Job::Branches {
            request: self.data.branch_request,
            root,
        })
    }

    /// The History view closed: a commit no longer needs to reload the list.
    pub fn leave_history(&mut self) {
        self.view.history.wanted = false;
    }

    /// Start loading history, once, for the History view.
    pub fn request_history(&mut self) {
        self.view.history.wanted = true;
        if matches!(self.data.commits.list, Load::Empty | Load::Error(_)) {
            self.data.commits.list = Load::Loading;
            self.data.commits.queued_page = Some(0);
        }
    }

    /// Load the page after the commits already listed.
    pub fn request_more_history(&mut self) {
        let Some(history) = self.data.commits.list.ready() else {
            return;
        };
        if history.more && !self.data.commits.loading_more {
            self.data.commits.loading_more = true;
            self.data.commits.queued_page = Some(history.commits.len());
        }
    }

    /// Show a commit's patch, from the cache when it was seen recently.
    pub fn select_commit(&mut self, id: String) {
        self.view.history.selected = Some(id.clone());
        if let Some(patch) = self.data.commits.shows.get(&id).cloned() {
            self.view.history.patch = Load::Ready(patch);
            self.view.history.queued_show = None;
        } else {
            self.view.history.patch = Load::Loading;
            self.view.history.queued_show = Some(id);
        }
    }

    /// History needs the root that a status reports.
    fn history_job(&mut self) -> Option<Job> {
        let root = self.data.root.clone()?;
        let epoch = self.data.commits.epoch;
        if let Some(skip) = self.data.commits.queued_page.take() {
            let tip = match &self.data.commits.list {
                Load::Ready(history) if skip > 0 => history.commits.first().map(|c| c.id.clone()),
                _ => None,
            };
            return Some(Job::Log {
                epoch,
                root,
                tip,
                skip,
            });
        }
        let id = self.view.history.queued_show.take()?;
        Some(Job::Show { epoch, root, id })
    }

    /// One job at a time. A write goes first so a click is never starved by
    /// reads; then status and diffs, then history, branches, and the file list.
    pub fn next_job(&mut self) -> Option<Job> {
        if self.data.running {
            return None;
        }
        let job = self
            .write_job()
            .or_else(|| self.data.pending.take())
            .or_else(|| self.history_job())
            .or_else(|| self.branches_job())
            .or_else(|| self.files_job())
            .or_else(|| self.data.prefetch.take())?;
        self.data.running = true;
        Some(job)
    }

    pub fn complete(&mut self, job: Job, reply: Reply) {
        self.data.running = false;
        match (job, reply) {
            (Job::Status { epoch, .. }, Reply::Status(result)) if epoch == self.data.epoch => {
                match result {
                    Ok(snapshot) => {
                        self.data.path = snapshot.root.clone();
                        let mode = snapshot.changes.first().map(|c| {
                            if c.unstaged() {
                                DiffMode::Unstaged
                            } else {
                                DiffMode::Staged
                            }
                        });
                        let remembered = self
                            .data
                            .write
                            .reselect
                            .take()
                            .or_else(|| self.view.changes.restore.take());
                        let had_selection = remembered.is_some();
                        let again = remembered.and_then(|(path, mode)| {
                            let file = snapshot.changes.iter().position(|c| c.path == path)?;
                            let change = &snapshot.changes[file];
                            // After staging, the file may only have a staged side left.
                            let mode = match mode {
                                DiffMode::Unstaged if !change.unstaged() => DiffMode::Staged,
                                DiffMode::Staged if !change.staged() => DiffMode::Unstaged,
                                mode => mode,
                            };
                            Some((file, mode))
                        });
                        self.data.staged = snapshot.changes.iter().filter(|c| c.staged()).count();
                        self.data.conflicts =
                            snapshot.changes.iter().filter(|c| c.conflicted()).count();
                        self.data.root = Some(snapshot.root.clone());
                        self.data.snapshot = Load::Ready(snapshot);
                        if let Some((file, mode)) = again {
                            self.select(file, mode);
                        } else if !had_selection && let Some(mode) = mode {
                            self.select(0, mode);
                        }
                    }
                    Err(error) => {
                        // No status means no root to list files under.
                        if std::mem::take(&mut self.data.files_queued) {
                            self.data.files = Load::Error(error.clone());
                        }
                        // Without a root, history and branches waiting on one can never load.
                        if self.data.root.is_none() {
                            self.fail_history(&error);
                            if std::mem::take(&mut self.data.branches_queued) {
                                self.data.branches = Load::Error(error.clone());
                            }
                        }
                        self.data.snapshot = Load::Error(error);
                    }
                }
            }
            (Job::Files { epoch, .. }, Reply::Files(result)) if epoch == self.data.epoch => {
                self.data.files = match result {
                    Ok(files) => Load::Ready(files),
                    Err(error) => Load::Error(error),
                };
            }
            (Job::Diff { epoch, key, .. }, Reply::Diff(result)) if epoch == self.data.epoch => {
                if let Ok(diff) = &result {
                    self.data.cache.insert(key.clone(), diff.clone());
                    self.bound_payload_cache();
                }
                if self.view.changes.selected.as_ref() == Some(&key) {
                    let ok = result.is_ok();
                    self.view.changes.diff = match result {
                        Ok(diff) => Load::Ready(diff),
                        Err(error) => Load::Error(error),
                    };
                    self.data.pending = None;
                    if ok {
                        self.queue_prefetch(&key);
                    }
                }
            }
            (Job::Write { root, op }, Reply::Write(result)) => {
                // A pull or a switch moves HEAD without a commit of ours;
                // history must reload.
                let moved = matches!(op, git::WriteOp::Pull | git::WriteOp::SwitchBranch(_))
                    && result.is_ok();
                self.data.write.running = false;
                if let Ok(outcome) = &result {
                    self.data.write.committed = outcome.commit.clone();
                    self.data.write.undo = outcome.backup.clone();
                }
                self.data.write.note = Some(match &result {
                    Ok(outcome) => (outcome.summary.clone(), false),
                    Err(error) => (error.clone(), true),
                });
                // The repository changed on disk whatever the view shows now.
                // Only a commit changes history; staging rereads status alone.
                if self.data.root.as_ref().is_none_or(|known| *known == root) {
                    let reselect = self.data.write.reselect.take();
                    self.refresh_status();
                    if self.data.write.committed.is_some() || moved {
                        self.reset_history();
                    }
                    self.data.write.reselect = reselect;
                }
            }
            (Job::Log { epoch, skip, .. }, Reply::Log(result))
                if epoch == self.data.commits.epoch =>
            {
                self.data.commits.loading_more = false;
                match (result, &mut self.data.commits.list) {
                    (Ok(page), Load::Ready(history)) if skip > 0 => {
                        history.commits.extend(page.commits);
                        history.more = page.more;
                    }
                    (Ok(page), list) => *list = Load::Ready(page),
                    // A failed later page keeps what is listed.
                    (Err(_), Load::Ready(_)) if skip > 0 => {}
                    (Err(error), list) => *list = Load::Error(error),
                }
            }
            (Job::Show { epoch, id, .. }, Reply::Show(result))
                if epoch == self.data.commits.epoch =>
            {
                if let Ok(patch) = &result {
                    self.data.commits.shows.insert(id.clone(), patch.clone());
                    self.bound_payload_cache();
                }
                if self.view.history.selected.as_ref() == Some(&id) {
                    self.view.history.patch = match result {
                        Ok(patch) => Load::Ready(patch),
                        Err(error) => Load::Error(error),
                    };
                }
            }
            (Job::Branches { request, .. }, Reply::Branches(result))
                if request == self.data.branch_request =>
            {
                self.data.branches = match result {
                    Ok(branches) => Load::Ready(branches),
                    Err(error) => Load::Error(error),
                };
            }
            (Job::Write { .. }, _) => self.data.write.running = false,
            _ => {}
        }
    }
}

impl RepositoryState {
    fn fail_history(&mut self, error: &str) {
        if self.data.commits.queued_page.take().is_some() {
            self.data.commits.list = Load::Error(error.to_owned());
        }
        if self.view.history.queued_show.take().is_some() {
            self.view.history.patch = Load::Error(error.to_owned());
        }
    }
}

pub struct AppState {
    pub projects: Vec<Project>,
    pub collapsed_projects: HashSet<usize>,
    pub collapsed_machines: HashSet<(usize, usize)>,
    pub repository: RepositoryState,
    checkouts: CheckoutRegistry,
}

/// The project's name is the checkout's folder name, or the whole path for `/`.
pub fn project_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

impl AppState {
    /// One project, the opened checkout, on this machine. Remote machines arrive with M3.
    #[cfg(feature = "local-host")]
    pub fn new(path: PathBuf) -> Self {
        Self {
            projects: vec![Project {
                checkout: path.clone(),
                name: project_name(&path),
                machines: vec![Machine {
                    name: machine::local_name(),
                    local: true,
                }],
            }],
            collapsed_projects: HashSet::new(),
            collapsed_machines: HashSet::new(),
            repository: RepositoryState::new(path),
            checkouts: CheckoutRegistry::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Change;
    use std::time::Duration;

    pub(super) trait CloneForTest {
        fn clone_for_test(&self) -> Snapshot;
    }

    impl CloneForTest for Snapshot {
        fn clone_for_test(&self) -> Snapshot {
            Snapshot {
                root: self.root.clone(),
                branch: self.branch.clone(),
                named: self.named,
                tracking: self.tracking.clone(),
                changes: self.changes.clone(),
                elapsed: self.elapsed,
                plain: self.plain,
            }
        }
    }

    pub(super) fn snapshot() -> Arc<Snapshot> {
        Arc::new(Snapshot {
            root: "/repo".into(),
            branch: "main".into(),
            named: true,
            tracking: None,
            changes: vec![
                Change {
                    path: "a".into(),
                    original: None,
                    index: b'M',
                    worktree: b'M',
                },
                Change {
                    path: "b".into(),
                    original: None,
                    index: b'?',
                    worktree: b'?',
                },
            ],
            elapsed: Duration::ZERO,
            plain: false,
        })
    }

    #[test]
    fn selected_paths_survive_reorder_but_not_rename_or_deletion() {
        let mut state = RepositoryState::new("/repo".into());
        let job = state.next_job().unwrap();
        state.complete(job, Reply::Status(Ok(snapshot())));
        state.select(1, DiffMode::Unstaged);
        state.refresh();
        let job = state.next_job().unwrap();
        let mut reordered = snapshot().clone_for_test();
        reordered.changes.reverse();
        state.complete(job, Reply::Status(Ok(Arc::new(reordered))));
        assert_eq!(
            state.view.changes.selected.as_ref().unwrap().path,
            PathBuf::from("b")
        );
        assert_eq!(state.view.changes.selected.as_ref().unwrap().file, 0);
        state.refresh();
        let job = state.next_job().unwrap();
        let mut renamed = snapshot().clone_for_test();
        renamed.changes[1].original = Some("b".into());
        renamed.changes[1].path = "renamed".into();
        state.complete(job, Reply::Status(Ok(Arc::new(renamed))));
        assert!(state.view.changes.selected.is_none());
        state.select(0, DiffMode::Staged);
        state.refresh();
        let job = state.next_job().unwrap();
        let mut deleted = snapshot().clone_for_test();
        deleted.changes.remove(0);
        state.complete(job, Reply::Status(Ok(Arc::new(deleted))));
        assert!(state.view.changes.selected.is_none());
    }

    #[test]
    fn stale_repository_result_cannot_replace_new_selection() {
        let mut s = RepositoryState::new("old".into());
        let old = s.next_job().unwrap();
        s.open("new".into());
        assert!(s.next_job().is_none());
        s.complete(old, Reply::Status(Ok(snapshot())));
        assert_eq!(s.path(), std::path::Path::new("new"));
        assert!(matches!(s.snapshot(), Load::Loading));
        assert!(
            matches!(s.next_job(), Some(Job::Status { path, .. }) if path == std::path::Path::new("new"))
        );
    }

    #[test]
    fn latest_diff_wins_and_cached_switch_has_no_job() {
        let mut s = RepositoryState::new("repo".into());
        let job = s.next_job().unwrap();
        s.complete(job, Reply::Status(Ok(snapshot())));
        let old = s.next_job().unwrap();
        s.select(1, DiffMode::Unstaged);
        s.select(0, DiffMode::Staged);
        assert!(s.next_job().is_none());
        s.complete(
            old,
            Reply::Diff(Ok(Arc::new(Diff::message("old", Duration::ZERO)))),
        );
        assert!(matches!(s.diff(), Load::Loading));
        let current = s.next_job().unwrap();
        assert!(matches!(current, Job::Diff { ref key, .. } if key.mode == DiffMode::Staged));
        s.complete(
            current,
            Reply::Diff(Ok(Arc::new(Diff::message("new", Duration::ZERO)))),
        );
        s.select(0, DiffMode::Unstaged);
        assert_eq!(&*s.diff().ready().unwrap().lines[0].text, "old");
        // A click reads nothing ahead.
        assert!(s.next_job().is_none());
    }

    /// Run the queued read-ahead and return which file it read.
    fn prefetched(s: &mut RepositoryState) -> usize {
        let job = s.next_job().expect("a read-ahead is queued");
        let Job::Diff { key, .. } = &job else {
            panic!("expected a diff read-ahead");
        };
        let file = key.file;
        s.complete(
            job,
            Reply::Diff(Ok(Arc::new(Diff::message("ahead", Duration::ZERO)))),
        );
        file
    }

    fn file_list(path: &str) -> Arc<git::FileList> {
        Arc::new(git::FileList {
            paths: vec![path.into()],
            ..git::FileList::default()
        })
    }

    #[test]
    fn file_list_waits_for_status_and_is_cached_until_refresh() {
        let mut s = RepositoryState::new("repo".into());
        s.request_files();
        assert!(matches!(s.files(), Load::Loading));
        let status = s.next_job().unwrap();
        assert!(matches!(status, Job::Status { .. }));
        s.complete(status, Reply::Status(Ok(snapshot())));
        // The first changed file's diff was queued by the status result.
        let diff = s.next_job().unwrap();
        assert!(matches!(diff, Job::Diff { .. }));
        assert!(s.next_job().is_none());
        s.complete(
            diff,
            Reply::Diff(Ok(Arc::new(Diff::message("diff", Duration::ZERO)))),
        );
        let files = s.next_job().unwrap();
        assert!(matches!(&files, Job::Files { root, .. } if root == std::path::Path::new("/repo")));
        s.complete(files, Reply::Files(Ok(file_list("a"))));
        assert_eq!(s.files().ready().unwrap().paths, ["a"]);
        assert!(matches!(s.diff(), Load::Ready(_)));
        s.request_files();
        assert!(s.next_job().is_none());

        let epoch = s.epoch();
        s.refresh();
        assert_ne!(s.epoch(), epoch);
        assert!(matches!(s.files(), Load::Empty));
    }

    #[test]
    fn stale_file_list_cannot_enter_a_new_epoch() {
        let mut s = RepositoryState::new("old".into());
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(
            diff,
            Reply::Diff(Ok(Arc::new(Diff::message("diff", Duration::ZERO)))),
        );
        s.request_files();
        let old = s.next_job().unwrap();
        s.open("new".into());
        s.request_files();
        s.complete(old, Reply::Files(Ok(file_list("old"))));
        assert!(matches!(s.files(), Load::Loading));
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(diff, Reply::Diff(Err("gone".into())));
        let current = s.next_job().unwrap();
        s.complete(current, Reply::Files(Ok(file_list("new"))));
        assert_eq!(s.files().ready().unwrap().paths, ["new"]);
    }

    #[test]
    fn file_list_reports_a_failed_status_or_listing() {
        let mut s = RepositoryState::new("repo".into());
        s.request_files();
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Err("not a repository".into())));
        assert!(matches!(s.files(), Load::Error(error) if error == "not a repository"));
        assert!(s.next_job().is_none());
        // A later request reports the same state without queueing work.
        s.data.files = Load::Empty;
        s.request_files();
        assert!(matches!(s.files(), Load::Error(_)));
        assert!(s.next_job().is_none());

        let mut s = RepositoryState::new("repo".into());
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(diff, Reply::Diff(Err("gone".into())));
        s.request_files();
        let files = s.next_job().unwrap();
        s.complete(files, Reply::Files(Err("too large".into())));
        assert!(matches!(s.files(), Load::Error(error) if error == "too large"));
        // An error is not cached: asking again retries.
        s.request_files();
        assert!(matches!(s.next_job(), Some(Job::Files { .. })));
    }

    fn ready(path: &str) -> RepositoryState {
        let mut s = RepositoryState::new(path.into());
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(
            diff,
            Reply::Diff(Ok(Arc::new(Diff::message("diff", Duration::ZERO)))),
        );
        s
    }

    fn outcome(summary: &str) -> Reply {
        Reply::Write(Ok(git::WriteOutcome {
            summary: summary.into(),
            commit: None,
            backup: None,
        }))
    }

    #[test]
    fn a_write_waits_for_a_status_runs_first_and_is_never_doubled() {
        let mut s = RepositoryState::new("repo".into());
        // No root yet: nothing to write to.
        assert!(!s.request_write(git::WriteOp::StageAll));
        let mut s = ready("repo");
        s.select(1, DiffMode::Unstaged);
        assert!(s.request_write(git::WriteOp::Stage(vec!["b".into()])));
        assert!(s.write().busy());
        assert!(!s.request_write(git::WriteOp::StageAll));
        // The queued diff for the selection waits behind the write.
        let write = s.next_job().unwrap();
        assert!(matches!(&write, Job::Write { root, .. } if root == std::path::Path::new("/repo")));
        assert!(s.next_job().is_none());
        s.complete(write, outcome("Staged 1 file"));
        assert!(!s.write().busy());
        assert_eq!(s.write().note, Some(("Staged 1 file".into(), false)));
        // The write refreshed status, and the staged file stays selected.
        let status = s.next_job().unwrap();
        assert!(matches!(status, Job::Status { .. }));
        let mut staged = (*snapshot()).clone_for_test();
        staged.changes[1].index = b'A';
        staged.changes[1].worktree = b' ';
        s.complete(status, Reply::Status(Ok(Arc::new(staged))));
        assert_eq!(
            s.selected_diff(),
            Some(&DiffKey {
                file: 1,
                path: "b".into(),
                mode: DiffMode::Staged
            })
        );
    }

    #[test]
    fn a_failed_write_reports_and_still_refreshes() {
        let mut s = ready("repo");
        assert!(s.request_write(git::WriteOp::Commit {
            message: "x".into()
        }));
        let write = s.next_job().unwrap();
        s.complete(write, Reply::Write(Err("hook failed".into())));
        assert_eq!(s.write().note, Some(("hook failed".into(), true)));
        assert!(matches!(s.next_job(), Some(Job::Status { .. })));
    }

    fn commit(id: &str) -> git::Commit {
        git::Commit {
            id: id.into(),
            short: id.into(),
            author: "A".into(),
            time: 0,
            subject: id.into(),
        }
    }

    #[test]
    fn history_pages_append_and_a_refresh_reloads_only_when_shown() {
        let mut s = ready("repo");
        s.request_history();
        let first = s.next_job().unwrap();
        assert!(matches!(first, Job::Log { skip: 0, .. }));
        s.complete(
            first,
            Reply::Log(Ok(git::History {
                commits: vec![commit("a"), commit("b")],
                more: true,
            })),
        );
        s.request_more_history();
        s.request_more_history();
        let next = s.next_job().unwrap();
        assert!(matches!(next, Job::Log { skip: 2, .. }));
        assert!(s.next_job().is_none());
        s.complete(
            next,
            Reply::Log(Ok(git::History {
                commits: vec![commit("c")],
                more: false,
            })),
        );
        let list = s.commit_list().ready().unwrap();
        assert_eq!(list.commits.len(), 3);
        assert!(!list.more && !s.commit_loading_more());

        // Shown history reloads after a refresh, once a status provides the root.
        s.refresh();
        assert!(matches!(s.commit_list(), Load::Loading));
        assert!(matches!(s.next_job(), Some(Job::Status { .. })));
    }

    #[test]
    fn staging_keeps_history_and_a_commit_reloads_it() {
        let mut s = ready("repo");
        s.request_history();
        let page = s.next_job().unwrap();
        s.complete(
            page,
            Reply::Log(Ok(git::History {
                commits: vec![commit("a")],
                more: false,
            })),
        );
        assert_eq!(s.staged(), 1);
        assert!(s.request_write(git::WriteOp::StageAll));
        let write = s.next_job().unwrap();
        s.complete(write, outcome("Staged all changes"));
        // Only status is reread; the listed commits stay.
        assert!(matches!(s.next_job(), Some(Job::Status { .. })));
        assert_eq!(s.commit_list().ready().unwrap().commits.len(), 1);

        let mut s = ready("repo");
        s.request_history();
        let page = s.next_job().unwrap();
        s.complete(page, Reply::Log(Ok(git::History::default())));
        assert!(s.request_write(git::WriteOp::Commit {
            message: "m".into()
        }));
        let write = s.next_job().unwrap();
        s.complete(
            write,
            Reply::Write(Ok(git::WriteOutcome {
                summary: "Committed".into(),
                commit: Some("abc".into()),
                backup: None,
            })),
        );
        assert!(matches!(s.commit_list(), Load::Loading));
        let status = s.next_job().unwrap();
        assert!(matches!(status, Job::Status { .. }));
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        assert!(matches!(diff, Job::Diff { .. }));
        s.complete(diff, Reply::Diff(Err("gone".into())));
        assert!(matches!(s.next_job(), Some(Job::Log { skip: 0, .. })));
    }

    #[test]
    fn a_stale_commit_patch_is_cached_but_not_shown() {
        let mut s = ready("repo");
        s.select_commit("a".into());
        let a = s.next_job().unwrap();
        s.select_commit("b".into());
        s.complete(
            a,
            Reply::Show(Ok(Arc::new(Diff::message("a", Duration::ZERO)))),
        );
        assert!(matches!(s.commit_patch(), Load::Loading));
        let b = s.next_job().unwrap();
        assert!(matches!(&b, Job::Show { id, .. } if id == "b"));
        s.complete(
            b,
            Reply::Show(Ok(Arc::new(Diff::message("b", Duration::ZERO)))),
        );
        s.select_commit("a".into());
        assert_eq!(&*s.commit_patch().ready().unwrap().lines[0].text, "a");
        assert!(s.next_job().is_none());
    }

    fn page(ids: &[&str], more: bool) -> Reply {
        Reply::Log(Ok(git::History {
            commits: ids.iter().map(|id| commit(id)).collect(),
            more,
        }))
    }

    #[test]
    fn later_history_pages_count_from_the_first_pages_newest_commit() {
        let mut s = ready("repo");
        s.request_history();
        let first = s.next_job().unwrap();
        assert!(matches!(
            &first,
            Job::Log {
                tip: None,
                skip: 0,
                ..
            }
        ));
        s.complete(first, page(&["new", "old"], true));
        s.request_more_history();
        let next = s.next_job().unwrap();
        assert!(matches!(&next, Job::Log { tip: Some(tip), skip: 2, .. } if tip == "new"));
    }

    #[test]
    fn a_write_runs_while_a_refresh_is_still_reading_status() {
        let mut s = ready("repo");
        s.refresh();
        assert!(s.snapshot().ready().is_none());
        assert!(s.request_write(git::WriteOp::StageAll));
        assert!(matches!(s.next_job(), Some(Job::Write { .. })));
    }

    #[test]
    fn a_write_queued_before_switching_checkouts_is_refused_with_a_note() {
        let mut s = ready("repo");
        assert!(s.request_write(git::WriteOp::StageAll));
        s.open("other".into());
        assert!(matches!(s.next_job(), Some(Job::Status { .. })));
        assert!(!s.write().busy());
        assert!(matches!(&s.write().note, Some((_, true))));
    }

    fn listed(names: &[&str]) -> Reply {
        Reply::Branches(Ok(Arc::new(git::Branches {
            list: names
                .iter()
                .map(|name| git::Branch {
                    name: (*name).into(),
                    current: false,
                    time: 0,
                    elsewhere: None,
                })
                .collect(),
            more: false,
        })))
    }

    #[test]
    fn branches_are_reread_on_each_request_and_only_the_newest_reply_lands() {
        let mut s = ready("repo");
        s.request_branches();
        let first = s.next_job().unwrap();
        assert!(
            matches!(&first, Job::Branches { root, .. } if root == std::path::Path::new("/repo"))
        );
        // Asked again before the first answer: the first is stale.
        s.request_branches();
        s.complete(first, listed(&["old"]));
        assert!(matches!(s.branches(), Load::Loading));
        let second = s.next_job().unwrap();
        s.complete(second, listed(&["main", "topic"]));
        assert_eq!(s.branches().ready().unwrap().list.len(), 2);
        // A refresh keeps the list; another checkout drops it and its reply.
        s.refresh();
        assert!(s.branches().ready().is_some());
        s.request_branches();
        let late = loop {
            match s.next_job().unwrap() {
                job @ Job::Branches { .. } => break job,
                job => s.complete(job, Reply::Status(Ok(snapshot()))),
            }
        };
        s.open("other".into());
        s.complete(late, listed(&["main"]));
        assert!(matches!(s.branches(), Load::Empty));
    }

    #[test]
    fn branches_waiting_on_a_failed_status_report_the_error() {
        let mut s = RepositoryState::new("repo".into());
        s.request_branches();
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Err("not a repository".into())));
        assert!(matches!(s.branches(), Load::Error(e) if e == "not a repository"));
        assert!(s.next_job().is_none());
        // Asked again with the error still standing: it is reported at once.
        s.request_branches();
        assert!(matches!(s.branches(), Load::Error(_)));
        assert!(s.next_job().is_none());
    }

    #[test]
    fn a_branch_switch_rereads_status_and_shown_history_but_a_new_branch_keeps_history() {
        let mut s = ready("repo");
        s.request_history();
        let first = s.next_job().unwrap();
        s.complete(first, page(&["a"], false));
        assert!(s.request_write(git::WriteOp::CreateBranch("topic".into())));
        let write = s.next_job().unwrap();
        s.complete(write, outcome("Created topic and switched to it"));
        assert!(matches!(s.snapshot(), Load::Loading));
        assert!(s.commit_list().ready().is_some(), "same commits");
        assert!(s.request_write(git::WriteOp::SwitchBranch("main".into())));
        let write = s.next_job().unwrap();
        s.complete(write, outcome("Switched to main"));
        assert!(matches!(s.commit_list(), Load::Loading));
        assert_eq!(s.write().note, Some(("Switched to main".into(), false)));
    }

    #[test]
    fn history_waiting_on_a_failed_status_reports_the_error() {
        let mut s = RepositoryState::new("repo".into());
        s.request_history();
        s.select_commit("a".into());
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Err("not a repository".into())));
        assert!(matches!(s.commit_list(), Load::Error(e) if e == "not a repository"));
        assert!(matches!(s.commit_patch(), Load::Error(_)));
        assert!(s.next_job().is_none());
    }

    #[test]
    fn leaving_history_stops_a_commit_from_rereading_it() {
        let mut s = ready("repo");
        s.request_history();
        let first = s.next_job().unwrap();
        s.complete(first, page(&["a"], false));
        s.leave_history();
        assert!(s.request_write(git::WriteOp::Commit {
            message: "m".into()
        }));
        let write = s.next_job().unwrap();
        s.complete(
            write,
            Reply::Write(Ok(git::WriteOutcome {
                summary: "Committed".into(),
                commit: Some("b".into()),
                backup: None,
            })),
        );
        assert!(matches!(s.commit_list(), Load::Empty));
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(diff, Reply::Diff(Err("gone".into())));
        assert!(s.next_job().is_none());
        // Coming back loads it fresh.
        s.request_history();
        assert!(matches!(s.next_job(), Some(Job::Log { skip: 0, .. })));
    }

    #[test]
    fn a_refresh_keeps_the_selected_commit_and_its_patch() {
        let mut s = ready("repo");
        s.request_history();
        let first = s.next_job().unwrap();
        s.complete(first, page(&["a"], false));
        s.select_commit("a".into());
        let show = s.next_job().unwrap();
        s.complete(
            show,
            Reply::Show(Ok(Arc::new(Diff::message("a", Duration::ZERO)))),
        );
        s.refresh();
        assert_eq!(s.selected_commit(), Some("a"));
        assert!(s.commit_patch().ready().is_some());
        // The list reloads; the patch is not read again.
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let diff = s.next_job().unwrap();
        s.complete(diff, Reply::Diff(Err("gone".into())));
        assert!(matches!(s.next_job(), Some(Job::Log { .. })));
        assert!(s.next_job().is_none());
    }

    #[test]
    fn conflicts_are_counted_once_per_status() {
        let mut s = RepositoryState::new("repo".into());
        let status = s.next_job().unwrap();
        let mut conflicted = (*snapshot()).clone_for_test();
        conflicted.changes[0].index = b'U';
        conflicted.changes[0].worktree = b'U';
        s.complete(status, Reply::Status(Ok(Arc::new(conflicted))));
        assert_eq!(s.conflicts(), 1);
        s.refresh();
        assert_eq!(s.conflicts(), 0);
    }

    #[test]
    fn stepping_reads_the_next_file_ahead_and_a_click_does_not() {
        let mut s = ready("repo");
        assert!(s.next_job().is_none());
        // Stepping forward to file 1 would read file 0 next (the list wraps),
        // which is cached already, so only file 1 itself is read.
        s.step(1, DiffMode::Unstaged, true);
        let diff = s.next_job().unwrap();
        s.complete(
            diff,
            Reply::Diff(Ok(Arc::new(Diff::message("one", Duration::ZERO)))),
        );
        assert!(s.next_job().is_none(), "file 0 is cached");
        // With an empty cache, a step reads the following file ahead.
        s.refresh();
        let status = s.next_job().unwrap();
        s.complete(status, Reply::Status(Ok(snapshot())));
        let first = s.next_job().unwrap();
        s.complete(first, Reply::Diff(Err("gone".into())));
        s.step(1, DiffMode::Unstaged, false);
        let one = s.next_job().unwrap();
        s.complete(
            one,
            Reply::Diff(Ok(Arc::new(Diff::message("one", Duration::ZERO)))),
        );
        assert_eq!(prefetched(&mut s), 0);
        s.step(0, DiffMode::Unstaged, false);
        assert_eq!(&*s.diff().ready().unwrap().lines[0].text, "ahead");
    }

    #[test]
    fn repository_data_and_view_scopes_keep_open_and_refresh_invariants() {
        let mut s = ready("repo");
        s.request_history();
        let log = s.next_job().unwrap();
        s.complete(log, page(&["c1"], false));
        s.select_commit("c1".into());
        let show = s.next_job().unwrap();
        s.complete(
            show,
            Reply::Show(Ok(Arc::new(Diff::message("patch", Duration::ZERO)))),
        );
        assert_eq!(s.selected_commit(), Some("c1"));
        assert!(s.commit_patch().ready().is_some());
        // Opening another checkout resets both checkout data and view selection,
        // while preserving whether History is wanted.
        s.open("other".into());
        assert_eq!(s.path(), std::path::Path::new("other"));
        assert!(s.selected_diff().is_none());
        assert!(matches!(s.diff(), Load::Empty));
        assert!(s.selected_commit().is_none());
        assert!(matches!(s.commit_patch(), Load::Empty));
        assert!(s.view.history.wanted);
        assert!(matches!(s.commit_list(), Load::Loading));
    }

    #[test]
    fn the_checkout_is_the_only_project_on_this_machine() {
        let s = AppState::new("/tmp/plait-fixture".into());
        assert_eq!(s.projects.len(), 1);
        assert_eq!(s.projects[0].name, "plait-fixture");
        assert_eq!(s.projects[0].machines.len(), 1);
        assert!(s.projects[0].machines[0].local);
        assert_eq!(project_name(std::path::Path::new("/")), "/");
    }
}
