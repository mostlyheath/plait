//! Local project navigation and activity tracking. No filesystem access or UI types.

use crate::{
    host::{ThreadStatus, ThreadSummary, harness::HarnessId},
    model::{Machine, Project},
    preferences::{Jump, JumpTarget},
    state::project_name,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Presentation state only. Thread content and execution still belong to the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub opened: Vec<PathBuf>,
    pub hidden: Vec<PathBuf>,
    pub observed: BTreeMap<String, Activity>,
    pub unread: BTreeSet<String>,
    pub mute_notifications: bool,
    /// Worktrees Plait created, by folder. Their threads list under the
    /// project they were made from rather than as projects of their own.
    pub worktrees: BTreeMap<PathBuf, WorktreeOf>,
}

/// Remembered worktrees beyond this drop out of grouping; their threads
/// then list under the worktree folder, as any other checkout.
const WORKTREE_LIMIT: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeOf {
    /// The repository checkout the worktree was created from.
    pub main: PathBuf,
    pub branch: String,
    /// Absent for older desktop-created worktrees: grouping, never deletion authority.
    pub creation_id: Option<String>,
    pub identity: Option<crate::git::worktree::Identity>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub updated_at: u64,
    pub state: String,
}

impl Activity {
    fn from_thread(thread: &ThreadSummary) -> Self {
        Self {
            updated_at: thread.updated_at.unwrap_or(0),
            state: state_label(thread).into(),
        }
    }
}

impl Preferences {
    /// The project a checkout lists under: the repository a Plait worktree was
    /// made from, or the checkout itself.
    pub fn project_of<'a>(&'a self, checkout: &'a Path) -> &'a Path {
        self.worktrees
            .get(checkout)
            .map_or(checkout, |tree| tree.main.as_path())
    }

    pub fn worktree(&self, checkout: &Path) -> Option<&WorktreeOf> {
        self.worktrees.get(checkout)
    }

    pub fn remember_worktree(&mut self, path: PathBuf, main: PathBuf, branch: String) {
        self.worktrees.insert(
            path,
            WorktreeOf {
                main,
                branch,
                creation_id: None,
                identity: None,
            },
        );
        self.normalize();
    }

    pub fn remember_host_worktree(&mut self, operation: &crate::host::worktree::Operation) {
        if let Some(tree) = &operation.worktree {
            self.worktrees.insert(
                tree.path.clone(),
                WorktreeOf {
                    main: tree.parent.main.clone(),
                    branch: tree.branch.clone(),
                    creation_id: Some(operation.request_id.clone()),
                    identity: tree.identity.clone(),
                },
            );
            self.normalize();
        }
    }

    /// Opening a worktree opens the project it belongs to.
    pub fn open(&mut self, checkout: &Path) {
        let checkout = &self.project_of(checkout).to_owned();
        self.hidden.retain(|path| path != checkout);
        if !self.opened.iter().any(|path| path == checkout) {
            self.opened.push(checkout.into());
        }
        self.normalize();
    }

    pub fn hide(&mut self, checkout: &Path) {
        let checkout = &self.project_of(checkout).to_owned();
        self.opened.retain(|path| path != checkout);
        if !self.hidden.iter().any(|path| path == checkout) {
            self.hidden.push(checkout.into());
        }
        self.normalize();
    }

    pub fn normalize(&mut self) {
        for paths in [&mut self.opened, &mut self.hidden] {
            let mut seen = HashSet::new();
            paths.retain(|path| {
                path.is_absolute() && path.as_os_str().len() <= 4096 && seen.insert(path.clone())
            });
            if paths.len() > 32 {
                paths.drain(..paths.len() - 32);
            }
        }
        self.observed
            .retain(|id, activity| id.len() <= 256 && activity.state.len() <= 32);
        if self.observed.len() > 1000 {
            let mut newest = self
                .observed
                .iter()
                .map(|(id, activity)| (activity.updated_at, id.clone()))
                .collect::<Vec<_>>();
            newest.sort();
            for (_, id) in newest.into_iter().take(self.observed.len() - 1000) {
                self.observed.remove(&id);
            }
        }
        self.unread.retain(|id| self.observed.contains_key(id));
        self.worktrees.retain(|path, tree| {
            path.is_absolute()
                && tree.main.is_absolute()
                && path != &tree.main
                && path.as_os_str().len() <= 4096
                && tree.main.as_os_str().len() <= 4096
                && tree.branch.len() <= 256
        });
        // A worktree of a worktree lists under the first one's project.
        let mains = self.worktrees.keys().cloned().collect::<HashSet<_>>();
        self.worktrees.retain(|_, tree| !mains.contains(&tree.main));
        while self.worktrees.len() > WORKTREE_LIMIT {
            self.worktrees.pop_first();
        }
    }

    /// Mark terminal/approval changes, but never notify from an initial listing.
    /// Persisted observations still make changes since the last launch unread.
    pub fn observe(
        &mut self,
        threads: &[ThreadSummary],
        visible: Option<&str>,
        initial: bool,
    ) -> Vec<String> {
        let mut notify = Vec::new();
        for thread in threads {
            let next = Activity::from_thread(thread);
            let previous = self.observed.insert(thread.id.clone(), next.clone());
            let changed = previous.as_ref().is_some_and(|old| old != &next);
            let attention = thread.pending_approval == Some(true)
                || matches!(
                    thread.status,
                    ThreadStatus::AwaitingApproval
                        | ThreadStatus::Error
                        | ThreadStatus::Interrupted
                        | ThreadStatus::Stopped
                )
                || (thread.status == ThreadStatus::Idle
                    && previous.as_ref().is_some_and(|old| {
                        matches!(old.state.as_str(), "Working" | "Needs approval")
                    }));
            if visible == Some(thread.id.as_str()) {
                self.unread.remove(&thread.id);
            } else if changed && attention {
                self.unread.insert(thread.id.clone());
                if !initial && !self.mute_notifications && thread.archived != Some(true) {
                    notify.push(thread.id.clone());
                }
            }
        }
        self.normalize();
        notify
    }
}

/// Overlay a bounded live listing on older loaded history. The protocol has
/// no delete operation, so leaving its newest-200 window is not deletion.
/// Validate the whole frame before mutating and ignore older pagination races.
pub fn merge_threads(
    threads: &mut Vec<ThreadSummary>,
    incoming: Vec<ThreadSummary>,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for next in &incoming {
        if next
            .checkout_queue
            .as_ref()
            .is_some_and(|queue| !queue.valid())
        {
            return Err("Invalid checkout queue summary.".into());
        }
        if next.id.is_empty() || !next.checkout.is_absolute() || !seen.insert(&next.id) {
            return Err(
                "Host returned invalid or duplicate thread identities. Reconnect to reload.".into(),
            );
        }
        if threads
            .iter()
            .any(|old| old.id == next.id && old.checkout != next.checkout)
        {
            return Err(
                "Host changed a thread's checkout. Reconnect to reload its identity.".into(),
            );
        }
    }
    for next in incoming {
        if let Some(old) = threads.iter_mut().find(|old| old.id == next.id) {
            if next.updated_at >= old.updated_at {
                *old = next;
            }
        } else {
            threads.push(next);
        }
    }
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
    Ok(())
}

pub fn state_label(thread: &ThreadSummary) -> &'static str {
    if !thread.status.busy()
        && let Some(queue) = &thread.checkout_queue
    {
        return queue.label();
    }
    if thread.pending_approval == Some(true) {
        return "Needs approval";
    }
    match thread.status {
        ThreadStatus::Running => "Working",
        ThreadStatus::Idle if thread.updated_at > thread.created_at => "Done",
        other => other.label(),
    }
}

pub fn needs_attention(thread: &ThreadSummary, unread: &BTreeSet<String>) -> bool {
    unread.contains(&thread.id)
        || thread.pending_approval == Some(true)
        || matches!(
            thread.status,
            ThreadStatus::AwaitingApproval | ThreadStatus::Error | ThreadStatus::Interrupted
        )
}

/// What a window does with its current folder once the selected machine's
/// projects are known. A folder belongs to the machine it was chosen on.
#[derive(Debug, PartialEq, Eq)]
pub enum Adoption {
    /// The folder is one of this machine's projects.
    Keep,
    /// It is not: open this project of the machine instead.
    Open(PathBuf),
    /// It is not, and the machine has no project to open.
    Unlisted,
}

pub fn adopt(current: &Path, preferences: &Preferences, projects: &[Project]) -> Adoption {
    let project = preferences.project_of(current);
    if projects.iter().any(|item| item.checkout == project) {
        Adoption::Keep
    } else if let Some(first) = projects.first() {
        Adoption::Open(first.checkout.clone())
    } else {
        Adoption::Unlisted
    }
}

/// Projects follow newest activity.
pub fn projects(
    threads: &[ThreadSummary],
    preferences: &Preferences,
    current: &Path,
    machine: &Machine,
    include_current: bool,
) -> Vec<Project> {
    let current = preferences.project_of(current);
    let mut activity = HashMap::new();
    for thread in threads {
        let project = preferences.project_of(&thread.checkout);
        let updated = activity.entry(project).or_insert(0);
        *updated = (*updated).max(thread.updated_at.unwrap_or(0));
    }
    let hidden = preferences
        .hidden
        .iter()
        .map(PathBuf::as_path)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut paths = threads
        .iter()
        .map(|thread| preferences.project_of(&thread.checkout))
        .chain(preferences.opened.iter().map(PathBuf::as_path))
        // Another machine's window must not list a checkout its host never named.
        .chain(include_current.then_some(current))
        .filter(|path| (!hidden.contains(path) || *path == current) && seen.insert(*path))
        .map(Path::to_owned)
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(activity.get(path.as_path()).copied().unwrap_or(0)));
    paths
        .into_iter()
        .map(|checkout| Project {
            name: project_name(&checkout),
            checkout,
            machines: vec![machine.clone()],
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRowState {
    Working,
    Approval,
    Failed,
    Interrupted,
    Queued(&'static str),
    FinishedUnread,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MachineActivity {
    pub working: usize,
    pub finished: usize,
    pub failed: usize,
    pub approval: usize,
    /// Index into the same revision of the thread list.
    pub next: Option<(usize, ThreadRowState)>,
}

pub fn machine_activity(threads: &[ThreadSummary], unread: &BTreeSet<String>) -> MachineActivity {
    use ThreadRowState::*;
    let mut activity = MachineActivity::default();
    let mut oldest = None;
    for (index, thread) in threads.iter().enumerate() {
        if thread.archived == Some(true) {
            continue;
        }
        let state = thread_row_state(thread, unread);
        let priority = match state {
            Approval => {
                activity.approval += 1;
                0
            }
            Failed => {
                activity.failed += 1;
                1
            }
            Interrupted => 1,
            FinishedUnread => {
                activity.finished += 1;
                2
            }
            Working => {
                activity.working += 1;
                continue;
            }
            Other | Queued(_) => continue,
        };
        // Measure waiting time by last activity. IDs break timestamp ties across refreshes.
        let key = (
            priority,
            thread.updated_at.or(thread.created_at),
            thread.id.as_str(),
        );
        if oldest.is_none_or(|previous| key < previous) {
            oldest = Some(key);
            activity.next = Some((index, state));
        }
    }
    activity
}

pub fn thread_row_state(thread: &ThreadSummary, unread: &BTreeSet<String>) -> ThreadRowState {
    use ThreadRowState::*;
    if thread.pending_approval == Some(true) {
        return Approval;
    }
    match thread.status {
        ThreadStatus::Running => Working,
        ThreadStatus::AwaitingApproval => Approval,
        ThreadStatus::Error => Failed,
        ThreadStatus::Interrupted => Interrupted,
        _ if let Some(queue) = &thread.checkout_queue => Queued(queue.label()),
        _ if unread.contains(&thread.id) => FinishedUnread,
        _ => Other,
    }
}

impl ThreadRowState {
    /// The state in words for an accessible name. It agrees with the row's
    /// visible word, and names a settled thread's state where the row shows
    /// only its age.
    pub fn spoken(self, thread: &ThreadSummary) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Approval => "Needs approval",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Queued(label) => label,
            Self::FinishedUnread | Self::Other => state_label(thread),
        }
    }

    /// The row's right-hand text.
    pub fn label(self, age: &str) -> &str {
        match self {
            Self::Working => "Working",
            Self::Approval => "Approve",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Queued(label) => label,
            Self::FinishedUnread | Self::Other => age,
        }
    }
}

/// The content of one sidebar card. Absent branch or details lines take no space.
pub struct Card<'a> {
    pub title: String,
    pub state: ThreadRowState,
    pub word: Option<&'static str>,
    pub spoken_state: &'static str,
    pub unread: bool,
    pub archived: bool,
    pub branch: Option<&'a str>,
    pub details: Option<CardDetails<'a>>,
}

pub struct CardDetails<'a> {
    pub harness: Option<HarnessId>,
    pub model: Option<&'a str>,
    pub age: Option<String>,
}

fn card_state(thread: &ThreadSummary, unread: &BTreeSet<String>) -> ThreadRowState {
    match thread_row_state(thread, unread) {
        ThreadRowState::FinishedUnread if state_label(thread) != "Done" => ThreadRowState::Other,
        state => state,
    }
}

impl<'a> Card<'a> {
    pub fn new(
        thread: &'a ThreadSummary,
        lists: &'a Preferences,
        title: String,
        model_name: Option<&'a str>,
        now: u64,
    ) -> Self {
        use ThreadRowState::*;
        let state = card_state(thread, &lists.unread);
        let word = match state {
            Approval => Some("Approve"),
            Failed => Some("Failed"),
            Interrupted => Some("Interrupted"),
            Queued(label) => Some(label),
            FinishedUnread => Some("Done"),
            Working | Other => None,
        };
        let branch = lists
            .worktree(&thread.checkout)
            .map(|tree| tree.branch.as_str())
            .filter(|branch| !branch.trim().is_empty());
        let harness = thread.harness;
        let model = model_name
            .filter(|name| !name.trim().is_empty())
            .or_else(|| Some(thread.model.model_id.as_str()).filter(|id| !id.trim().is_empty()));
        let age = thread.updated_at.map(|updated| age(Some(updated), now));
        Self {
            title,
            state,
            word,
            spoken_state: state.spoken(thread),
            unread: lists.unread.contains(&thread.id),
            archived: thread.archived == Some(true),
            branch,
            details: (harness.is_some() || model.is_some() || age.is_some()).then_some(
                CardDetails {
                    harness,
                    model,
                    age,
                },
            ),
        }
    }
}

/// Sort one project's cards, including the order used by sidebar shortcuts.
pub fn sort_cards(indices: &mut [usize], threads: &[ThreadSummary], unread: &BTreeSet<String>) {
    use ThreadRowState::*;
    indices.sort_unstable_by_key(|&index| {
        let thread = &threads[index];
        let priority = match card_state(thread, unread) {
            Approval => 0,
            Failed | Interrupted => 1,
            FinishedUnread => 2,
            Working => 3,
            Queued(_) | Other => 4,
        };
        (priority, Reverse(thread.updated_at), thread.id.as_str())
    });
}

pub fn needs_me_count(threads: &[ThreadSummary], unread: &BTreeSet<String>) -> usize {
    threads
        .iter()
        .filter(|t| t.archived != Some(true) && needs_attention(t, unread))
        .count()
}

pub fn jump(
    preferences: &Preferences,
    projects: &[Project],
    threads: &[ThreadSummary],
    indices: &[usize],
    target: JumpTarget,
    number: usize,
) -> Option<Jump> {
    let number = number.checked_sub(1)?;
    let grouped = projects
        .iter()
        .enumerate()
        .map(|(project, p)| {
            indices
                .iter()
                .copied()
                .filter(move |&index| {
                    preferences.project_of(&threads[index].checkout) == p.checkout
                })
                .map(move |thread| Jump {
                    project,
                    machine: 0,
                    thread,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    match target {
        JumpTarget::Threads => grouped.into_iter().flatten().nth(number),
        JumpTarget::Projects | JumpTarget::Machines => grouped.get(number)?.first().copied(),
    }
}

pub fn age(updated_at: Option<u64>, now: u64) -> String {
    let Some(updated_at) = updated_at else {
        return String::new();
    };
    let minutes = now.saturating_sub(updated_at) / 60_000;
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn thread(id: &str, checkout: &str, status: ThreadStatus, updated: u64) -> ThreadSummary {
        ThreadSummary {
            id: id.into(),
            checkout: checkout.into(),
            status,
            created_at: Some(1),
            updated_at: Some(updated),
            ..Default::default()
        }
    }
    #[test]
    fn machine_activity_counts_and_visits_in_attention_order() {
        use ThreadStatus::*;
        let mut threads = vec![
            thread("working", "/one", Running, 1),
            thread("done-b", "/one", Idle, 2),
            thread("done-a", "/one", Idle, 2),
            thread("failed", "/one", Error, 4),
            thread("interrupted", "/one", Interrupted, 3),
            thread("approval-new", "/one", AwaitingApproval, 6),
            thread("approval-old", "/one", AwaitingApproval, 5),
        ];
        let unread = BTreeSet::from(["done-a".into(), "done-b".into()]);
        let activity = machine_activity(&threads, &unread);
        assert_eq!(
            (
                activity.working,
                activity.finished,
                activity.failed,
                activity.approval
            ),
            (1, 2, 1, 2)
        );
        for id in [
            "approval-old",
            "approval-new",
            "interrupted",
            "failed",
            "done-a",
            "done-b",
        ] {
            let index = machine_activity(&threads, &unread).next.unwrap().0;
            assert_eq!(threads[index].id, id);
            threads[index].archived = Some(true);
        }
        assert_eq!(
            machine_activity(&threads, &unread),
            MachineActivity {
                working: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn live_overlay_adds_reorders_and_preserves_older_rows_without_regression() {
        let mut threads = vec![
            thread("a", "/one", ThreadStatus::Running, 20),
            thread("b", "/two", ThreadStatus::Idle, 10),
        ];
        merge_threads(
            &mut threads,
            vec![
                thread("c", "/three", ThreadStatus::Error, 40),
                thread("a", "/one", ThreadStatus::Idle, 30),
            ],
        )
        .unwrap();
        assert_eq!(
            threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        merge_threads(
            &mut threads,
            vec![thread("a", "/one", ThreadStatus::Running, 20)],
        )
        .unwrap();
        assert_eq!(threads[1].status, ThreadStatus::Idle);
        let before = threads.clone();
        assert!(
            merge_threads(
                &mut threads,
                vec![thread("a", "/wrong", ThreadStatus::Error, 50)]
            )
            .is_err()
        );
        assert_eq!(threads, before);
        assert!(
            merge_threads(
                &mut threads,
                vec![
                    thread("new", "/r", ThreadStatus::Idle, 50),
                    thread("new", "/r", ThreadStatus::Idle, 50)
                ]
            )
            .is_err()
        );
        assert_eq!(threads, before);
    }

    #[test]
    fn activity_marks_changes_once_and_survives_restart() {
        let mut preferences = Preferences::default();
        let mut t = thread("a", "/repo", ThreadStatus::Running, 1);
        assert!(preferences.observe(&[t.clone()], None, true).is_empty());
        t.status = ThreadStatus::Idle;
        t.updated_at = Some(2);
        assert_eq!(preferences.observe(&[t.clone()], None, false), ["a"]);
        assert!(preferences.unread.contains("a"));
        let saved = serde_json::to_string(&preferences).unwrap();
        let mut restored: Preferences = serde_json::from_str(&saved).unwrap();
        assert!(restored.observe(&[t.clone()], None, true).is_empty());
        assert!(restored.unread.contains("a"));
        restored.observe(&[t], Some("a"), false);
        assert!(restored.unread.is_empty());
    }
    #[test]
    fn visible_and_muted_changes_do_not_notify() {
        let mut p = Preferences::default();
        let mut t = thread("a", "/r", ThreadStatus::Running, 1);
        p.observe(&[t.clone()], None, true);
        t.status = ThreadStatus::AwaitingApproval;
        t.updated_at = Some(2);
        assert!(p.observe(&[t.clone()], Some("a"), false).is_empty());
        assert!(p.unread.is_empty());
        p.mute_notifications = true;
        t.status = ThreadStatus::Error;
        t.updated_at = Some(3);
        assert!(p.observe(&[t], None, false).is_empty());
        assert!(p.unread.contains("a"));
    }
    fn local() -> Machine {
        Machine {
            name: "Local".into(),
            local: true,
        }
    }

    #[test]
    fn another_machines_window_lists_only_what_its_host_named() {
        let p = Preferences {
            opened: vec!["/srv/a".into()],
            ..Default::default()
        };
        let names = |include| {
            projects(&[], &p, Path::new("/Users/me/mac-only"), &local(), include)
                .into_iter()
                .map(|project| project.checkout)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(false), vec![PathBuf::from("/srv/a")]);
        assert_eq!(names(true).len(), 2);
        assert!(
            projects(
                &[],
                &Preferences::default(),
                Path::new("/x"),
                &local(),
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn adoption_respects_machine_projects_and_worktree_grouping() {
        let mut preferences = Preferences::default();
        preferences.open(Path::new("/local"));
        preferences.remember_worktree("/local-tree".into(), "/local".into(), "branch".into());
        let listed = projects(&[], &preferences, Path::new("/remote"), &local(), false);
        assert_eq!(
            adopt(Path::new("/local"), &preferences, &listed),
            Adoption::Keep
        );
        assert_eq!(
            adopt(Path::new("/local-tree"), &preferences, &listed),
            Adoption::Keep
        );
        assert_eq!(
            adopt(Path::new("/remote"), &preferences, &listed),
            Adoption::Open("/local".into())
        );
        assert_eq!(
            adopt(Path::new("/remote"), &preferences, &[]),
            Adoption::Unlisted
        );
        // A hidden project is not a place to land.
        preferences.hide(Path::new("/local"));
        let hidden = projects(
            &[thread("a", "/local", ThreadStatus::Idle, 1)],
            &preferences,
            Path::new("/remote"),
            &local(),
            false,
        );
        assert!(hidden.is_empty());
        assert_eq!(
            adopt(Path::new("/remote"), &preferences, &hidden),
            Adoption::Unlisted
        );
    }

    #[test]
    fn carried_checkout_is_not_added_to_projects() {
        let preferences = Preferences {
            opened: vec!["/local".into()],
            ..Default::default()
        };
        let listed = projects(&[], &preferences, Path::new("/remote"), &local(), false);
        assert_eq!(
            listed
                .iter()
                .map(|project| project.checkout.as_path())
                .collect::<Vec<_>>(),
            vec![Path::new("/local")]
        );
    }

    #[test]
    fn projects_are_grouped_and_hidden_without_deleting_threads() {
        let threads = [
            thread("a", "/one", ThreadStatus::Idle, 1),
            thread("b", "/two", ThreadStatus::Running, 2),
        ];
        let mut p = Preferences::default();
        p.open(Path::new("/empty"));
        let groups = projects(&threads, &p, Path::new("/one"), &local(), true);
        assert_eq!(
            groups.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["two", "one", "empty"]
        );
        assert_eq!(
            jump(
                &Preferences::default(),
                &groups,
                &threads,
                &[0, 1],
                JumpTarget::Threads,
                1
            )
            .unwrap()
            .thread,
            1
        );
        assert_eq!(
            jump(
                &Preferences::default(),
                &groups,
                &threads,
                &[0, 1],
                JumpTarget::Projects,
                2
            )
            .unwrap()
            .thread,
            0
        );
        assert!(
            jump(
                &Preferences::default(),
                &groups,
                &threads,
                &[0, 1],
                JumpTarget::Projects,
                3
            )
            .is_none()
        );
        p.hide(Path::new("/two"));
        assert_eq!(
            projects(&threads, &p, Path::new("/one"), &local(), true).len(),
            2
        );
        assert_eq!(threads.len(), 2);
    }
    #[test]
    fn sidebar_states_and_attention_count() {
        use crate::host::checkout_queue::{State, Summary};
        use ThreadRowState::*;
        let unread = BTreeSet::from(["unread".to_owned()]);
        let mut threads = Vec::new();
        for (status, id, pending, queued, expected, label, word) in [
            (
                ThreadStatus::Running,
                "unread",
                false,
                true,
                Working,
                "Working",
                None,
            ),
            (
                ThreadStatus::Running,
                "unread",
                true,
                false,
                Approval,
                "Approve",
                Some("Approve"),
            ),
            (
                ThreadStatus::AwaitingApproval,
                "a",
                false,
                false,
                Approval,
                "Approve",
                Some("Approve"),
            ),
            (
                ThreadStatus::Error,
                "unread",
                false,
                true,
                Failed,
                "Failed",
                Some("Failed"),
            ),
            (
                ThreadStatus::Interrupted,
                "unread",
                false,
                false,
                Interrupted,
                "Interrupted",
                Some("Interrupted"),
            ),
            (
                ThreadStatus::Idle,
                "unread",
                false,
                true,
                Queued("Held"),
                "Held",
                Some("Held"),
            ),
            (
                ThreadStatus::Idle,
                "unread",
                false,
                false,
                FinishedUnread,
                "3m",
                Some("Done"),
            ),
            (ThreadStatus::Idle, "b", false, false, Other, "3m", None),
        ] {
            let mut t = thread(id, "/repo", status, 2);
            t.pending_approval = Some(pending);
            t.checkout_queue = queued.then(|| Summary::new("request".into(), State::Held, 1, 1));
            let state = thread_row_state(&t, &unread);
            assert_eq!(state, expected);
            assert_eq!(state.label("3m"), label);
            let lists = Preferences {
                unread: unread.clone(),
                ..Default::default()
            };
            let card = Card::new(&t, &lists, "Card title".into(), None, 180_002);
            assert_eq!(card.state, expected);
            assert_eq!(card.word, word);
            assert_eq!(card.unread, id == "unread");
            assert_eq!(card.title, "Card title");
            assert_eq!(card.details.unwrap().age.as_deref(), Some("3m"));
            // The spoken state never contradicts the visible word.
            let spoken = state.spoken(&t);
            match state {
                Approval => assert_eq!(spoken, "Needs approval"),
                FinishedUnread | Other => assert_eq!(spoken, state_label(&t)),
                _ => assert_eq!(spoken, label),
            }
            threads.push(t);
        }
        threads[0].archived = Some(true);
        assert_eq!(needs_me_count(&threads, &unread), 6);
        for t in threads {
            assert_eq!(
                needs_me_count(std::slice::from_ref(&t), &unread) == 1,
                t.archived != Some(true) && needs_attention(&t, &unread)
            );
        }
    }

    #[test]
    fn cards_omit_missing_data_and_use_only_the_threads_own_worktree_branch() {
        let mut lists = Preferences::default();
        lists.remember_worktree("/worktree".into(), "/repo".into(), "plait/retry".into());
        let mut t = ThreadSummary {
            checkout: "/repo".into(),
            ..Default::default()
        };
        let card = Card::new(&t, &lists, "Old thread".into(), None, 120_000);
        assert_eq!(card.branch, None);
        assert!(card.details.is_none());
        assert_eq!(card.word, None);
        t.checkout = "/worktree".into();
        t.harness = Some(HarnessId::Codex);
        t.model.model_id = "codex-model".into();
        t.archived = Some(true);
        let card = Card::new(&t, &lists, "Retry".into(), Some("Codex Model"), 120_000);
        assert_eq!(card.branch, Some("plait/retry"));
        assert!(card.archived);
        let details = card.details.unwrap();
        assert_eq!(details.model, Some("Codex Model"));
        assert_eq!(details.harness, Some(HarnessId::Codex));
        assert_eq!(details.age, None);
        let card = Card::new(&t, &lists, "Retry".into(), None, 120_000);
        assert_eq!(card.details.unwrap().model, Some("codex-model"));

        t.status = ThreadStatus::Stopped;
        lists.unread.insert(t.id.clone());
        let card = Card::new(&t, &lists, "Stopped".into(), None, 120_000);
        assert_eq!(card.state, ThreadRowState::Other);
        assert_eq!(card.spoken_state, "Stopped");
        assert!(card.unread);
    }

    #[test]
    fn cards_sort_by_state_then_last_update_with_stable_ties() {
        use crate::host::checkout_queue::{State, Summary};
        use ThreadStatus::*;
        let mut threads = [
            thread("working", "/repo", Running, 500),
            thread("failed", "/repo", Error, 100),
            thread("approval", "/repo", AwaitingApproval, 2),
            thread("idle-z", "/repo", Idle, 800),
            thread("interrupted", "/repo", Interrupted, 101),
            thread("finished", "/repo", Idle, 3),
            thread("idle-a", "/repo", Idle, 800),
            thread("queued", "/repo", Idle, 900),
            ThreadSummary::default(),
        ];
        threads[7].checkout_queue = Some(Summary::new("request".into(), State::Waiting, 1, 1));
        let unread = BTreeSet::from(["finished".to_owned()]);
        let mut indices = (0..threads.len()).collect::<Vec<_>>();
        sort_cards(&mut indices, &threads, &unread);
        assert_eq!(indices, [2, 4, 1, 5, 0, 7, 6, 3, 8]);
        indices.reverse();
        sort_cards(&mut indices, &threads, &unread);
        assert_eq!(indices, [2, 4, 1, 5, 0, 7, 6, 3, 8]);
    }
    #[test]
    fn worktree_threads_list_under_the_project_they_were_made_from() {
        let mut preferences = Preferences::default();
        preferences.remember_worktree(
            "/repo.worktrees/a1".into(),
            "/repo".into(),
            "plait/a1".into(),
        );
        let threads = [
            thread("w", "/repo.worktrees/a1", ThreadStatus::Running, 30),
            thread("o", "/other", ThreadStatus::Idle, 20),
            thread("m", "/repo", ThreadStatus::Idle, 10),
        ];
        let list = projects(
            &threads,
            &preferences,
            Path::new("/repo.worktrees/a1"),
            &local(),
            true,
        );
        let checkouts = list
            .iter()
            .map(|p| p.checkout.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            checkouts,
            ["/repo", "/other"],
            "newest activity, worktree folded in"
        );
        let jump = jump(
            &preferences,
            &list,
            &threads,
            &[0, 1, 2],
            JumpTarget::Threads,
            2,
        )
        .unwrap();
        assert_eq!((jump.project, jump.thread), (0, 2));
        // Opening the worktree opens its project.
        preferences.open(Path::new("/repo.worktrees/a1"));
        assert_eq!(preferences.opened, [PathBuf::from("/repo")]);
        assert_eq!(
            preferences
                .worktree(Path::new("/repo.worktrees/a1"))
                .unwrap()
                .branch,
            "plait/a1"
        );
        // Nested and self-referential entries never chain.
        preferences.remember_worktree(
            "/repo.worktrees/b2".into(),
            "/repo.worktrees/a1".into(),
            "plait/b2".into(),
        );
        preferences.remember_worktree("/same".into(), "/same".into(), "x".into());
        assert_eq!(preferences.worktrees.len(), 1);
    }
    #[test]
    fn older_preferences_and_future_timestamps_are_safe() {
        assert_eq!(
            serde_json::from_str::<Preferences>("{}").unwrap(),
            Preferences::default()
        );
        assert_eq!(age(Some(120_000), 60_000), "now");
        assert_eq!(age(Some(0), 60_000 * 61), "1h");
        assert_eq!(age(None, 0), "");
    }
}

#[cfg(test)]
mod imported_worktrees {
    use super::*;
    #[test]
    fn old_desktop_worktrees_are_imported_without_deletion_authority() {
        let old = r#"{"worktrees":{"/repo.worktrees/old":{"main":"/repo","branch":"plait/old"}}}"#;
        let preferences: Preferences = serde_json::from_str(old).unwrap();
        let tree = preferences
            .worktree(Path::new("/repo.worktrees/old"))
            .unwrap();
        assert!(tree.creation_id.is_none());
        assert!(tree.identity.is_none());
        assert_eq!(
            preferences.project_of(Path::new("/repo.worktrees/old")),
            Path::new("/repo")
        );
    }
}
