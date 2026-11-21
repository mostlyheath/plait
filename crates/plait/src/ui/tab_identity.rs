//! Identity is verified off the UI thread; an older host keeps session-only tabs.
use super::{Plait, tab_lifecycle::Intent};
use gpui::*;
use plait_core::{
    host::{self},
    tabs::{CheckoutKey, DirectoryIdentity, HostKey, TabTarget},
};
use std::{collections::HashMap, os::unix::fs::MetadataExt, path::PathBuf};

#[derive(Default)]
pub(super) struct Identity {
    pub verified: bool,
    verified_epoch: Option<u64>,
    pub pending: Option<(HostKey, Vec<CheckoutKey>)>,
    pub checkouts: HashMap<PathBuf, DirectoryIdentity>,
    pub catalog_ready: bool,
    pub generation: u64,
    pub opening: u64,
}

pub(super) fn directory(root: &std::path::Path) -> Option<DirectoryIdentity> {
    let meta = std::fs::metadata(root).ok()?;
    meta.is_dir().then_some(DirectoryIdentity {
        device: meta.dev(),
        inode: meta.ino(),
    })
}

impl Plait {
    pub(super) fn tab_identity_current(&self) -> bool {
        !self.tabs.identity.verified
            || self.tabs.identity.verified_epoch == Some(self.host.connection.epoch)
    }

    pub(super) fn verify_tab_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        self.tabs.identity.generation += 1;
        let generation = self.tabs.identity.generation;
        let epoch = self.host.connection.epoch;
        let thread = self.host.connection.threads.first().map(|t| t.id.clone());
        let mut roots: Vec<_> = self
            .host
            .connection
            .threads
            .iter()
            .map(|t| t.checkout.clone())
            .collect();
        roots.extend(self.preferences.value.navigation.opened.iter().cloned());
        roots.push(self.host.creation.checkout.clone());
        roots.extend(
            self.tabs
                .model
                .tabs()
                .iter()
                .map(|t| t.session().target().checkout().root.clone()),
        );
        if let Some(data) = &self.session.data {
            roots.extend(data.draft_tabs.values().map(|tab| tab.checkout.clone()));
            roots.extend(data.new_drafts.keys().cloned());
            roots.extend(data.new_draft.as_ref().map(|(root, _)| root.clone()));
        }
        roots.sort();
        roots.dedup();
        let client = self.host_client();
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let host = host::identity::read(&client).ok().or_else(|| {
                let info = host::review::info(&client, thread.as_deref()?).ok()?;
                let (_, store) = plait_core::session::locations().ok()?;
                host::identity::from_review(info.checkout.host_id, store).ok()
            })?;
            let checkouts = roots
                .into_iter()
                .filter_map(|root| {
                    // The remote host verifies its own checkout identity.
                    if local {
                        directory(&root).map(|directory| CheckoutKey {
                            host: host.clone(),
                            root,
                            directory: Some(directory),
                        })
                    } else {
                        Some(CheckoutKey {
                            host: host.clone(),
                            root,
                            directory: None,
                        })
                    }
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
            self.tabs.model.disconnected(&previous);
            self.capture_session(cx);
            self.park_workspace(cx);
            self.notice = Some("The host store changed. Open its threads from the sidebar; previous tabs remain offline.".into());
        }
        self.tabs.host = host;
        self.tabs.identity.verified = true;
        self.tabs.identity.verified_epoch = Some(self.host.connection.epoch);
        self.tabs.identity.checkouts = checkouts
            .into_iter()
            .filter_map(|c| c.directory.map(|d| (c.root, d)))
            .collect();
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
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let root = if local && worktrees {
                plait_core::git::worktree::main_checkout(&root).unwrap_or(root)
            } else {
                root
            };
            // Remote checkout identity is checked by its host.
            let identity = if local { directory(&root) } else { None };
            (root, identity)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (root, directory) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.tabs.identity.opening != opening || this.tabs.host != host {
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
