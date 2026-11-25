//! Checkout identity is verified by the host off the UI thread.
use super::{Plait, tab_lifecycle::Intent};
use gpui::*;
use plait_client::{
    host::{self},
    tabs::{CheckoutKey, DirectoryIdentity, HostKey, TabTarget},
};
use std::{collections::HashMap, path::PathBuf};

#[derive(Default)]
pub(super) struct Identity {
    pub verified: bool,
    verified_epoch: Option<u64>,
    /// The machine selected when `verified` was set. A switch changes the
    /// selected machine before its link and identity are made.
    pub machine: Option<plait_client::machines::MachineId>,
    pub pending: Option<(HostKey, Vec<CheckoutKey>)>,
    pub checkouts: HashMap<PathBuf, DirectoryIdentity>,
    /// Checkouts a remote host vouched for. This Mac cannot stat them, so they
    /// carry no directory identity, but their tabs still restore.
    pub remote: Vec<PathBuf>,
    pub catalog_ready: bool,
    pub generation: u64,
    pub opening: u64,
    pub recovery: Option<(PathBuf, bool)>,
    pub reopening: bool,
}

#[cfg(test)]
pub(super) fn directory(root: &std::path::Path) -> Option<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;
    if root.canonicalize().ok()?.as_path() != root {
        return None;
    }
    let meta = std::fs::symlink_metadata(root).ok()?;
    meta.is_dir().then_some(DirectoryIdentity {
        device: meta.dev(),
        inode: meta.ino(),
    })
}

impl Plait {
    pub(super) fn refresh_checkout_identity(
        &mut self,
        old: &plait_client::ownership::CheckoutKey,
        refreshed: &plait_client::ownership::CheckoutKey,
        cx: &mut Context<Self>,
    ) {
        if old == refreshed {
            return;
        }
        if self.state.refresh_checkout(old, refreshed) {
            self.pump(cx);
        }
        self.host.refresh_checkout(old, refreshed);
        let previous = DirectoryIdentity {
            device: old.device,
            inode: old.inode,
        };
        if self.tabs.identity.checkouts.get(&old.root) != Some(&previous) {
            return;
        }
        let directory = DirectoryIdentity {
            device: refreshed.device,
            inode: refreshed.inode,
        };
        let key = CheckoutKey {
            host: self.tabs.host.clone(),
            root: refreshed.root.clone(),
            directory: Some(directory),
        };
        if self.tabs.model.reopen_checkout(&key).is_err() {
            return;
        }
        self.tabs.lifecycle.reopen_checkout(&key);
        self.tabs
            .identity
            .checkouts
            .insert(refreshed.root.clone(), directory);
        self.tabs.persistence.edited = true;
        self.capture_session(cx);
    }

    pub(super) fn identity_message(path: &std::path::Path) -> String {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        format!("{name} changed on disk since Plait opened it. It may have been moved or replaced.")
    }

    pub(super) fn has_identity_recovery(&self, message: &str) -> bool {
        self.tabs
            .identity
            .recovery
            .as_ref()
            .is_some_and(|(path, _)| Self::identity_message(path) == message)
    }

    pub(super) fn identity_recovery_message(&self, fallback: &str) -> String {
        match &self.tabs.identity.recovery {
            Some((path, true)) if Self::identity_message(path) == fallback => {
                format!("{} no longer exists.", path.display())
            }
            _ => fallback.to_owned(),
        }
    }

    pub(super) fn identity_recovery_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use super::theme::*;
        let missing = self
            .tabs
            .identity
            .recovery
            .as_ref()
            .is_some_and(|(_, missing)| *missing);
        let busy = self.tabs.identity.reopening;
        button(
            "identity-recovery",
            if missing {
                "Open project…"
            } else if busy {
                "Reopening…"
            } else {
                "Reopen project"
            },
        )
        .disabled(busy)
        .on_activate(cx, move |this, window, cx| {
            if missing {
                this.open_repository(&super::OpenRepository, window, cx);
            } else {
                this.reopen_project(window, cx);
            }
        })
    }

    pub(super) fn reopen_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, _)) = self.tabs.identity.recovery.clone() else {
            return;
        };
        if self.tabs.identity.reopening {
            return;
        }
        self.tabs.identity.reopening = true;
        self.tabs.identity.opening += 1;
        let generation = self.tabs.identity.opening;
        let host = self.tabs.host.clone();
        let epoch = self.host.connection.epoch;
        let supported = self.host.connection.has_checkout_identity;
        let client = self.host_client();
        let root = path.clone();
        let owner = self
            .state
            .checkout_key()
            .and_then(|key| key.host.clone())
            .or_else(|| {
                self.host
                    .thread
                    .selected
                    .as_deref()
                    .and_then(|id| self.host.expected_checkout(id))
                    .and_then(|key| key.host.clone())
            });
        let task = cx.background_executor().spawn(async move {
            let resolved = match host::workspace::open_project(&client, &root) {
                Err(host::Error::Rejected(reason)) if reason.code == "checkout_missing" => {
                    return Ok(None);
                }
                result => result?,
            };
            // The host reports the folder's identity; the desktop never reads it itself.
            let key = if supported {
                let reply = host::git::identity(&client, &resolved, None)?;
                Some(plait_client::ownership::CheckoutKey {
                    host: owner,
                    root: resolved.clone(),
                    device: reply.identity.dev,
                    inode: reply.identity.ino,
                })
            } else {
                None
            };
            let identity = key.as_ref().map(|key| DirectoryIdentity {
                device: key.device,
                inode: key.inode,
            });
            Ok::<_, host::Error>(Some((resolved, identity, key)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.tabs.identity.opening != generation
                    || this.tabs.host != host
                    || this.host.connection.epoch != epoch
                {
                    this.tabs.identity.reopening = false;
                    cx.notify();
                    return;
                }
                this.tabs.identity.reopening = false;
                if this.tabs.identity.recovery.as_ref().map(|(root, _)| root) != Some(&path)
                    || this.host.creation.checkout != path
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Some((root, directory, owner_key))) if root == path => {
                        let key = CheckoutKey {
                            host,
                            root: root.clone(),
                            directory,
                        };
                        if let Err(error) = this.tabs.model.reopen_checkout(&key) {
                            this.notice = Some(error.to_string());
                            cx.notify();
                            return;
                        }
                        if let Some(owner_key) = owner_key {
                            this.state.reopen_checkout(owner_key.clone());
                            this.host.reopen_checkout(&owner_key);
                        }
                        this.tabs.lifecycle.reopen_checkout(&key);
                        if let Some(directory) = directory {
                            this.tabs.identity.checkouts.insert(root, directory);
                        }
                        let message = Self::identity_message(&path);
                        this.tabs.identity.recovery = None;
                        if this.host.connection.error.as_ref() == Some(&message) {
                            this.host.connection.error = None;
                        }
                        if this.draft.error.as_ref() == Some(&message) {
                            this.draft.error = None;
                        }
                        for draft in this.tabs.lifecycle.drafts.values_mut() {
                            if draft.error.as_ref() == Some(&message) {
                                draft.error = None;
                            }
                        }
                        if this
                            .workspace
                            .thread_controls
                            .note
                            .as_ref()
                            .is_some_and(|(_, note, _)| note == &message)
                        {
                            this.workspace.thread_controls.note = None;
                        }
                        this.tabs.persistence.edited = true;
                        this.capture_session(cx);
                    }
                    Ok(None) => this.tabs.identity.recovery = Some((path, true)),
                    Ok(Some(_)) => {
                        this.notice = Some(
                            "The project resolved to a different path. Open project… to choose it."
                                .into(),
                        )
                    }
                    Err(error) => this.notice = Some(format!("Could not reopen project: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Scenes have no host to ask: treat `host` as this connection's verified
    /// identity for the selected machine, as a completed check would.
    #[cfg(test)]
    pub(super) fn verify_identity_for_test(&mut self, host: HostKey) {
        self.tabs.host = host;
        self.tabs.identity.verified = true;
        self.tabs.identity.verified_epoch = Some(self.host.connection.epoch);
        self.tabs.identity.machine = Some(self.machines.active.clone());
    }

    pub(super) fn tab_identity_current(&self) -> bool {
        !self.tabs.identity.verified
            || self.tabs.identity.verified_epoch == Some(self.host.connection.epoch)
    }

    /// Saved draft folders the selected machine's host is asked to identify.
    /// Another machine's folders are not this host's to see or vouch for.
    pub(super) fn saved_draft_roots(&self) -> Vec<PathBuf> {
        let Some(data) = &self.session.data else {
            return Vec::new();
        };
        // The path-only fields kept for older builds are left out: they mirror
        // drafts of any machine, and loading turns each older build's entry
        // into a draft tab, which names its machine.
        data.draft_tabs
            .values()
            .filter(|tab| tab.maybe_on(&self.machines.active))
            .map(|tab| tab.checkout.clone())
            .collect()
    }

    pub(super) fn verify_tab_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        if !self.host.connection.has_checkout_identity {
            self.notice = Some("Update the host to verify the checkout.".into());
            cx.notify();
            return;
        }
        self.tabs.identity.generation += 1;
        let generation = self.tabs.identity.generation;
        let epoch = self.host.connection.epoch;
        let thread = self.host.connection.threads().first().map(|t| t.id.clone());
        let mut roots: Vec<_> = self
            .host
            .connection
            .threads()
            .iter()
            .map(|t| t.checkout.clone())
            .collect();
        roots.extend(self.lists().opened.iter().cloned());
        // A folder carried over from the previous machine is not this host's.
        if !self.host_workspace.checkout_carried {
            roots.push(self.host.creation.checkout.clone());
        }
        roots.extend(
            self.tabs
                .model
                .tabs()
                .iter()
                .map(|t| t.session().target().checkout().root.clone()),
        );
        roots.extend(self.saved_draft_roots());
        roots.sort();
        roots.dedup();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let host = host::identity::read(&client).ok().or_else(|| {
                let info = host::review::info(&client, thread.as_deref()?).ok()?;
                let (_, store) = plait_client::session::locations().ok()?;
                host::identity::from_review(info.checkout.host_id, store).ok()
            })?;
            let checkouts = roots
                .into_iter()
                .filter_map(|root| {
                    let result = host::git::identity(&client, &root, None).ok()?;
                    Some(CheckoutKey {
                        host: host.clone(),
                        root: result.result.root,
                        directory: Some(DirectoryIdentity {
                            device: result.identity.dev,
                            inode: result.identity.ino,
                        }),
                    })
                })
                .collect::<Vec<_>>();
            Some((host, checkouts))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if generation != this.tabs.identity.generation
                    || epoch != this.host.connection.epoch
                {
                    return;
                }
                let Some((host, checkouts)) = result else {
                    return;
                };
                if this.tabs.lifecycle.busy || this.tabs.lifecycle.hydrating {
                    this.tabs.identity.pending = Some((host, checkouts));
                } else {
                    this.finish_tab_identity(host, checkouts, window, cx);
                }
            });
        })
        .detach();
    }

    fn finish_tab_identity(
        &mut self,
        host: HostKey,
        checkouts: Vec<CheckoutKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.install_tab_identity(host, checkouts, cx);
        self.sync_composer(cx);
        self.try_restore_tabs(window, cx);
        self.restore_thread_selection(window, cx);
        let owned = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .is_some_and(|t| t.session().target().checkout().host == self.tabs.host);
        if owned
            && self.workspace.host_watch.is_none()
            && let Some(id) = self.host.thread.selected.clone()
        {
            self.watch_thread(id, window, cx);
            self.refresh_turn_review(cx);
        }
    }
    pub(super) fn apply_pending_tab_identity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.lifecycle.busy
            && !self.tabs.lifecycle.hydrating
            && let Some((host, checkouts)) = self.tabs.identity.pending.take()
        {
            self.finish_tab_identity(host, checkouts, window, cx);
        }
    }

    pub(super) fn install_tab_identity(
        &mut self,
        host: HostKey,
        checkouts: Vec<CheckoutKey>,
        cx: &mut Context<Self>,
    ) {
        let previous = self.tabs.host.clone();
        if !self.tabs.identity.verified {
            let result = self
                .tabs
                .model
                .verify_session_host(&previous, &host, &checkouts);
            if !self.tab_error(result) {
                return;
            }
            self.tabs
                .lifecycle
                .verify_pending(&previous, &host, &checkouts);
            for (target, _) in &mut self.tabs.lifecycle.editors {
                let checkout = match target {
                    TabTarget::Existing { checkout, .. }
                    | TabTarget::Draft { checkout, .. }
                    | TabTarget::Worktrees { checkout }
                    | TabTarget::Issues { checkout } => checkout,
                };
                if checkout.host == previous {
                    checkout.host = host.clone();
                    checkout.directory = checkouts
                        .iter()
                        .find(|c| c.root == checkout.root)
                        .and_then(|c| c.directory);
                }
            }
        } else if previous != host {
            let had_tabs = !self.tabs.model.tabs().is_empty();
            self.tabs.model.disconnected(&previous);
            self.capture_session(cx);
            self.forget_saved_work();
            self.park_workspace(cx);
            // A machine switch already put the previous machine's tabs away.
            if had_tabs {
                self.notice = Some("The host store changed. Open its threads from the sidebar; previous tabs remain offline.".into());
            }
        }
        self.tabs.host = host;
        self.tabs.identity.verified = true;
        self.tabs.identity.verified_epoch = Some(self.host.connection.epoch);
        self.tabs.identity.machine = Some(self.machines.active.clone());
        self.tabs.identity.remote = checkouts
            .iter()
            .filter(|c| c.directory.is_none())
            .map(|c| c.root.clone())
            .collect();
        self.tabs.identity.checkouts = checkouts
            .into_iter()
            .filter_map(|c| c.directory.map(|d| (c.root, d)))
            .collect();
        // The owner is verified now: bring back its saved sends and text.
        self.adopt_saved_work(cx);
        if self.host.connection.connected {
            let result = self
                .tabs
                .model
                .reconnected(&self.tabs.host, self.host.connection.epoch);
            self.tab_error(result);
        } else {
            self.tabs.model.disconnected(&self.tabs.host);
        }
        self.refresh_tab_rows();
        if self.worktrees_visible() {
            self.refresh_worktrees_view(false, cx);
        }
        cx.notify();
    }

    pub(super) fn open_tab_target(
        &mut self,
        mut target: TabTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (!self.tabs.identity.verified && !target.is_worktrees()) || cfg!(test) {
            self.request_tab(Intent::Open(target), window, cx);
            return;
        }
        self.tabs.identity.opening += 1;
        let opening = self.tabs.identity.opening;
        let host = self.tabs.host.clone();
        let root = target.checkout().root.clone();
        let worktrees = target.is_worktrees();
        let client = self.host_client();
        let epoch = self.host.connection.epoch;
        let supported = self.host.connection.has_checkout_identity;
        if !supported {
            self.notice = Some("Update the host to verify the checkout.".into());
            cx.notify();
            return;
        }
        let task = cx.background_executor().spawn(async move {
            let root = if worktrees {
                host::git::main_checkout(&client, &root, None)
                    .map(|read| read.result)
                    .unwrap_or(root)
            } else {
                root
            };
            let identity = host::git::identity(&client, &root, None).ok();
            match identity {
                Some(read) => (
                    read.result.root,
                    Some(DirectoryIdentity {
                        device: read.identity.dev,
                        inode: read.identity.ino,
                    }),
                ),
                None => (root, None),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let (root, directory) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.tabs.identity.opening != opening
                    || this.tabs.host != host
                    || this.host.connection.epoch != epoch
                {
                    return;
                }
                let checkout = match &mut target {
                    TabTarget::Existing { checkout, .. }
                    | TabTarget::Draft { checkout, .. }
                    | TabTarget::Worktrees { checkout }
                    | TabTarget::Issues { checkout } => checkout,
                };
                checkout.root = root;
                checkout.directory = directory;
                if let Some(directory) = directory {
                    this.tabs
                        .identity
                        .checkouts
                        .insert(checkout.root.clone(), directory);
                }
                this.request_tab(Intent::Open(target), window, cx);
            });
        })
        .detach();
    }
}
