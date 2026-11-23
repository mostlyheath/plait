//! The selected host's workspace: its identity, projects, worktree ownership,
//! and seen state, read from the host's stream. This Mac's saved lists are
//! used only with a local host too old to keep them, and are imported into
//! a newer local host once.
use super::{host::WatchLease, jump::PreferenceGroup, *};
use plait_client::{
    host::{
        self as host_api,
        workspace::{self, HostIdentity, Seen, View},
    },
    navigation::{self, Activity, WorktreeOf},
};

/// Where the selected machine's lists come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Source {
    /// Connecting: a remote machine shows nothing yet; this Mac shows its saved lists.
    #[default]
    Pending,
    /// The host keeps them, and names itself.
    Host(HostIdentity),
    /// The host is too old to keep them.
    Legacy,
}

#[derive(Default)]
pub(super) struct HostWorkspace {
    pub(super) source: Source,
    /// The host's lists, or, for an older remote host, only what this window opened.
    pub(super) lists: navigation::Preferences,
    pub(super) revision: u64,
    /// The host's change counters for the checkouts it watches.
    status: std::collections::BTreeMap<PathBuf, u64>,
    generation: u64,
    watch: Option<WatchLease>,
    importing: bool,
    /// The stream failed for a reason other than an older host.
    failed: bool,
}

/// The host's view as the sidebar's lists. Mute is this device's choice.
pub(super) fn lists_from(view: &View, mute: bool) -> navigation::Preferences {
    let mut lists = navigation::Preferences {
        mute_notifications: mute,
        ..Default::default()
    };
    for project in &view.projects {
        if project.hidden {
            lists.hidden.push(project.path.clone());
        } else {
            lists.opened.push(project.path.clone());
        }
    }
    for tree in &view.worktrees {
        lists.worktrees.insert(
            tree.folder.clone(),
            WorktreeOf {
                main: tree.main.clone(),
                branch: tree.branch.clone(),
                creation_id: tree.creation_id.clone(),
                identity: tree.identity.clone(),
            },
        );
    }
    for seen in &view.seen {
        lists.observed.insert(
            seen.thread_id.clone(),
            Activity {
                updated_at: seen.updated_at,
                state: seen.state.clone(),
            },
        );
        if seen.unread {
            lists.unread.insert(seen.thread_id.clone());
        }
    }
    lists
}

/// Checkouts whose counter moved or that started being watched.
pub(super) fn changed_checkouts(
    before: &std::collections::BTreeMap<PathBuf, u64>,
    after: &std::collections::BTreeMap<PathBuf, u64>,
) -> Vec<PathBuf> {
    after
        .iter()
        .filter(|(path, count)| before.get(*path) != Some(*count))
        .map(|(path, _)| path.clone())
        .collect()
}

/// What a change to the lists asks the host to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Changes {
    pub(super) open: Vec<PathBuf>,
    pub(super) hide: Vec<(PathBuf, bool)>,
    pub(super) seen: Vec<Seen>,
}

impl Changes {
    pub(super) fn is_empty(&self) -> bool {
        self.open.is_empty() && self.hide.is_empty() && self.seen.is_empty()
    }
}

/// The intents between two versions of the lists. Worktree ownership is the
/// host's own record, and a project leaves the list only by being hidden.
pub(super) fn changes(
    before: &navigation::Preferences,
    after: &navigation::Preferences,
) -> Changes {
    let mut out = Changes::default();
    for path in &after.opened {
        if !before.opened.contains(path) {
            out.open.push(path.clone());
        }
    }
    for path in &after.hidden {
        if !before.hidden.contains(path) {
            out.hide.push((path.clone(), true));
        }
    }
    for path in &before.hidden {
        if !after.hidden.contains(path) {
            out.hide.push((path.clone(), false));
        }
    }
    let ids = after
        .observed
        .keys()
        .chain(before.unread.iter())
        .chain(after.unread.iter())
        .collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        let (was, now) = (before.observed.get(id), after.observed.get(id));
        let unread = after.unread.contains(id);
        if was == now && before.unread.contains(id) == unread {
            continue;
        }
        if let Some(activity) = now {
            out.seen.push(Seen {
                thread_id: id.clone(),
                updated_at: activity.updated_at,
                state: activity.state.clone(),
                unread,
            });
        }
    }
    out
}

/// This device's saved lists in the host's form, for a one-time import.
fn import_lists(saved: &navigation::Preferences) -> (Vec<workspace::Worktree>, Vec<Seen>) {
    let worktrees = saved
        .worktrees
        .iter()
        .map(|(folder, tree)| workspace::Worktree {
            folder: folder.clone(),
            main: tree.main.clone(),
            branch: tree.branch.clone(),
            creation_id: tree.creation_id.clone(),
            identity: tree.identity.clone(),
        })
        .collect();
    let seen = saved
        .observed
        .iter()
        .map(|(id, activity)| Seen {
            thread_id: id.clone(),
            updated_at: activity.updated_at,
            state: activity.state.clone(),
            unread: saved.unread.contains(id),
        })
        .collect();
    (worktrees, seen)
}

impl Plait {
    /// The selected machine's project lists, unread state, and worktree owners.
    pub(super) fn lists(&self) -> &navigation::Preferences {
        match self.host_workspace.source {
            Source::Host(_) => &self.host_workspace.lists,
            _ if self.host_is_local() => &self.preferences.value.navigation,
            _ => &self.host_workspace.lists,
        }
    }

    #[cfg(test)]
    pub(super) fn host_workspace_generation(&self) -> u64 {
        self.host_workspace.generation
    }

    pub(super) fn host_workspace_started(&self) -> bool {
        self.host_workspace.generation > 0
    }

    pub(super) fn host_workspace_failed(&self) -> bool {
        self.host_workspace.failed
    }

    /// The machine's own name, once its host has said it.
    pub(super) fn host_identity(&self) -> Option<&HostIdentity> {
        match &self.host_workspace.source {
            Source::Host(identity) => Some(identity),
            _ => None,
        }
    }

    /// Changes the selected machine's lists. A host that keeps them is told;
    /// an older local host's lists are saved on this Mac as before.
    pub(super) fn update_lists(
        &mut self,
        change: impl FnOnce(&mut navigation::Preferences),
        cx: &mut Context<Self>,
    ) {
        let before = self.lists().clone();
        let mut after = before.clone();
        change(&mut after);
        if after == before {
            return;
        }
        self.navigation.invalidate_search();
        // A draw can reuse the parent view without visiting the visible list.
        self.navigation.update_cached_list_flags(&after, cx);
        match self.host_workspace.source {
            Source::Host(_) => {
                let changes = changes(&before, &after);
                self.host_workspace.lists = after;
                self.send_changes(changes, cx);
            }
            _ if self.host_is_local() => {
                let mut preferences = self.preferences.value.clone();
                preferences.navigation = after;
                self.persist_navigation(preferences, cx);
            }
            // An older remote host keeps nothing; this window remembers until it switches.
            _ => self.host_workspace.lists = after,
        }
        cx.notify();
    }

    fn send_changes(&mut self, changes: Changes, cx: &mut Context<Self>) {
        if changes.is_empty() {
            return;
        }
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            for path in &changes.open {
                workspace::open_project(&client, path)?;
            }
            for (path, hidden) in &changes.hide {
                workspace::hide_project(&client, path, *hidden)?;
            }
            for chunk in changes.seen.chunks(500) {
                workspace::mark_seen(&client, chunk)?;
            }
            Ok::<_, host_api::Error>(())
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                // The host's next view restores its lists; say why they changed back.
                let _ = this.update(cx, |this, cx| {
                    this.navigation.error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Follows the selected machine's workspace. A resubscription keeps the
    /// last view on screen until the new snapshot replaces it; switching
    /// machines clears it first.
    pub(super) fn watch_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = &mut self.host_workspace;
        workspace.generation += 1;
        workspace.watch = None;
        workspace.importing = false;
        let generation = workspace.generation;
        let executor = cx.background_executor().clone();
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { workspace::watch(&client) });
        cx.spawn_in(window, async move |this, cx| {
            let mut views = match task.await {
                Ok(views) => views,
                Err(error) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.workspace_failed(generation, &error, window, cx)
                    });
                    return;
                }
            };
            let lease = WatchLease {
                cancel: Some(views.cancel_handle()),
                executor,
            };
            let kept = this
                .update(cx, |this, _| {
                    let current = this.host_workspace.generation == generation;
                    if current {
                        this.host_workspace.watch = Some(lease);
                    }
                    current
                })
                .unwrap_or(false);
            if !kept {
                return;
            }
            while let Some(result) = views.next().await {
                let keep = this
                    .update_in(cx, |this, window, cx| match result {
                        Ok(view) => this.apply_workspace(generation, view, window, cx),
                        Err(error) => {
                            this.workspace_failed(generation, &error, window, cx);
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn workspace_failed(
        &mut self,
        generation: u64,
        error: &host_api::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host_workspace.generation != generation {
            return;
        }
        self.host_workspace.watch = None;
        if workspace::needs_host_update(error) {
            self.host_workspace.source = Source::Legacy;
            if self.host_is_local() {
                self.navigation
                    .update_cached_list_flags(&self.preferences.value.navigation, cx);
            }
        } else {
            self.host_workspace.failed = true;
        }
        // Tabs wait for the workspace; without one, they use this Mac's file.
        self.try_restore_tabs(window, cx);
        self.rebuild_projects(cx);
        cx.notify();
    }

    pub(super) fn apply_workspace(
        &mut self,
        generation: u64,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.host_workspace.generation != generation {
            return false;
        }
        if self.import_workspace(generation, &view, cx) {
            // The host answers the import with a new view.
            return true;
        }
        let mute = self.preferences.value.navigation.mute_notifications;
        let changed = changed_checkouts(&self.host_workspace.status, &view.status);
        let lists = lists_from(&view, mute);
        self.navigation.update_cached_list_flags(&lists, cx);
        let workspace = &mut self.host_workspace;
        workspace.lists = lists;
        workspace.revision = view.revision;
        workspace.status = view.status.clone();
        workspace.source = Source::Host(view.host);
        workspace.failed = false;
        self.update_window_title(window);
        if !self.tabs.persistence.host_loaded && !self.tabs.persistence.host_loading {
            self.tabs.persistence.host_loading = true;
            self.load_host_tabs(window, cx);
        }
        self.rebuild_projects(cx);
        self.adopt_host_project(window, cx);
        self.reread_changed(&changed, cx);
        self.refresh_tab_rows();
        cx.notify();
        true
    }

    /// The host saw files change in a checkout on screen: read it again. No
    /// view polls; this, focus, and Refresh are what reread.
    fn reread_changed(&mut self, changed: &[PathBuf], cx: &mut Context<Self>) {
        let open = self.state.repository.path().to_owned();
        if changed.contains(&open)
            && !matches!(
                self.state.repository.snapshot(),
                plait_client::state::Load::Loading
            )
        {
            self.state.repository.refresh();
            self.pump(cx);
        }
        if changed.contains(&self.workspace.file_viewer.root) {
            self.check_file(cx);
        }
    }

    /// A window that just moved to another machine opens one of that
    /// machine's projects, never a path it only had on the previous one.
    fn adopt_host_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host_is_local() {
            return;
        }
        let current = self.project_of(&self.host.creation.checkout).to_owned();
        if self.state.projects.iter().any(|p| p.checkout == current) {
            return;
        }
        if let Some(first) = self.state.projects.first().map(|p| p.checkout.clone()) {
            self.change_checkout(first, window, cx);
        }
    }

    /// The first time this Mac's own host keeps the lists, it receives the
    /// ones this Mac saved. Paths on this Mac mean nothing to another machine.
    fn import_workspace(&mut self, generation: u64, view: &View, cx: &mut Context<Self>) -> bool {
        let saved = &self.preferences.value;
        let empty = saved.navigation.opened.is_empty()
            && saved.navigation.hidden.is_empty()
            && saved.navigation.worktrees.is_empty()
            && saved.navigation.observed.is_empty();
        if !self.host_is_local()
            || empty
            || self.host_workspace.importing
            || saved.imported_workspaces.contains(&view.host.id)
        {
            return false;
        }
        self.host_workspace.importing = true;
        let mut preferences = saved.clone();
        if preferences.device_id.is_empty() {
            preferences.device_id = plait_client::host_state::request_id();
        }
        let device = preferences.device_id.clone();
        let projects = preferences.navigation.opened.clone();
        let hidden = preferences.navigation.hidden.clone();
        let (worktrees, seen) = import_lists(&preferences.navigation);
        let host_id = view.host.id.clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            workspace::import(
                &client,
                &workspace::Import {
                    device: &device,
                    projects: &projects,
                    hidden: &hidden,
                    worktrees: &worktrees,
                    seen: &seen,
                },
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host_workspace.generation != generation {
                    return;
                }
                this.host_workspace.importing = false;
                match result {
                    Ok(_) => {
                        // Recorded even when another window imported first: the host has them.
                        preferences.imported_workspaces.push(host_id);
                        this.set_preferences(preferences, PreferenceGroup::View, cx);
                    }
                    Err(error) => {
                        this.navigation.error =
                            Some(format!("Projects were not moved to the host: {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_client::host::workspace::Project;

    fn view() -> View {
        View {
            schema: 1,
            host: HostIdentity {
                id: "h".into(),
                name: "workstation".into(),
                platform: "linux".into(),
            },
            revision: 4,
            projects: vec![
                Project {
                    path: "/srv/a".into(),
                    hidden: false,
                    ..Default::default()
                },
                Project {
                    path: "/srv/b".into(),
                    hidden: true,
                    ..Default::default()
                },
            ],
            worktrees: vec![workspace::Worktree {
                folder: "/srv/a.worktrees/t".into(),
                main: "/srv/a".into(),
                branch: "plait/t".into(),
                creation_id: Some("c".into()),
                identity: None,
            }],
            seen: vec![Seen {
                thread_id: "t1".into(),
                updated_at: 9,
                state: "Ready".into(),
                unread: true,
            }],
            status: Default::default(),
        }
    }

    #[core::prelude::v1::test]
    fn a_host_view_becomes_the_sidebar_lists() {
        let lists = lists_from(&view(), true);
        assert_eq!(lists.opened, vec![PathBuf::from("/srv/a")]);
        assert_eq!(lists.hidden, vec![PathBuf::from("/srv/b")]);
        assert_eq!(
            lists.project_of(std::path::Path::new("/srv/a.worktrees/t")),
            std::path::Path::new("/srv/a")
        );
        assert!(lists.unread.contains("t1"));
        assert_eq!(lists.observed["t1"].updated_at, 9);
        assert!(lists.mute_notifications, "mute stays this device's choice");
    }

    #[core::prelude::v1::test]
    fn list_changes_become_host_intents() {
        let before = lists_from(&view(), false);
        let mut after = before.clone();
        after.open(std::path::Path::new("/srv/c"));
        after.hide(std::path::Path::new("/srv/a"));
        after.unread.remove("t1");
        after.observed.insert(
            "t2".into(),
            Activity {
                updated_at: 3,
                state: "Working".into(),
            },
        );
        let found = changes(&before, &after);
        assert_eq!(found.open, vec![PathBuf::from("/srv/c")]);
        assert!(found.hide.contains(&(PathBuf::from("/srv/a"), true)));
        let seen = |id: &str| found.seen.iter().find(|s| s.thread_id == id).cloned();
        assert!(!seen("t1").unwrap().unread);
        assert_eq!(seen("t2").unwrap().state, "Working");
        assert!(changes(&before, &before).is_empty());
        // Showing a hidden project again is an intent too.
        let shown = changes(&after, &before);
        assert!(shown.hide.contains(&(PathBuf::from("/srv/a"), false)));
    }

    #[core::prelude::v1::test]
    fn moved_and_new_counters_are_changes() {
        let map = |pairs: &[(&str, u64)]| {
            pairs
                .iter()
                .map(|(p, n)| (PathBuf::from(p), *n))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before = map(&[("/a", 1), ("/b", 4)]);
        let after = map(&[("/a", 2), ("/b", 4), ("/c", 0)]);
        assert_eq!(
            changed_checkouts(&before, &after),
            vec![PathBuf::from("/a"), PathBuf::from("/c")]
        );
        assert!(changed_checkouts(&after, &after).is_empty());
    }

    #[core::prelude::v1::test]
    fn saved_lists_import_in_the_hosts_form() {
        let lists = lists_from(&view(), false);
        let (worktrees, seen) = import_lists(&lists);
        assert_eq!(worktrees, view().worktrees);
        assert_eq!(seen, view().seen);
    }
}
