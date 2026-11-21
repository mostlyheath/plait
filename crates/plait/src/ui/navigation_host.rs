//! Cross-checkout thread summaries and system notification dispatch.

use super::{Plait, host::WatchLease};
use gpui::*;
use plait_core::{
    host::{self, Client, Request, ThreadSummary},
    navigation,
};
use std::collections::HashSet;

impl Plait {
    pub(super) fn initialize_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.navigation.filter.clone();
        cx.subscribe(&field, |_, _, _, cx| cx.notify()).detach();
        cx.observe_window_activation(window, |this, window, cx| {
            // Another app may have changed the open file.
            if window.is_window_active() {
                this.check_file(cx);
                if this.workspace.tab == super::Workspace::Changes {
                    this.refresh_turn_review(cx);
                }
            }
            if window.is_window_active()
                && !this.settings_open
                && this.workspace.tab == super::Workspace::Conversation
                && this.conversation_snapshot().is_some()
                && let Some(id) = this.host.thread.selected.clone()
            {
                this.mark_thread_read(&id, cx);
            }
        })
        .detach();
        if cfg!(test) {
            return;
        }
        let view = cx.weak_entity();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let Some(id) = response.tag.strip_prefix("plait-thread:") else {
                return;
            };
            let id = id.to_owned();
            let _ = handle.update(cx, |_, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this
                        .host
                        .connection
                        .threads
                        .iter()
                        .any(|thread| thread.id == id)
                    {
                        this.select_host_thread(id, window, cx);
                        window.activate_window();
                    }
                });
            });
        });
    }

    pub(super) fn mark_thread_read(&mut self, id: &str, cx: &mut Context<Self>) {
        self.dismiss_toast(id, cx);
        if self.preferences.value.navigation.unread.contains(id) {
            let mut preferences = self.preferences.value.clone();
            preferences.navigation.unread.remove(id);
            self.persist_navigation(preferences, cx);
            self.refresh_tab_rows();
        }
        if !cfg!(test) {
            cx.dismiss_system_notification(&format!("plait-thread:{id}"))
                .detach();
        }
    }

    pub(super) fn watch_all_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_host_worktrees(cx);
        self.navigation.watch = None;
        self.admission.queue = false;
        self.navigation.generation += 1;
        let generation = self.navigation.generation;
        self.navigation.loading = true;
        self.navigation.error = None;
        let executor = cx.background_executor().clone();
        let task = cx.background_executor().spawn(async move {
            let client = Client::from_environment()?;
            // Subscribe first, so changes during pagination remain in the inbox.
            let queue = host::checkout_queue::available(&client)?;
            let lists = client.subscribe_threads_queue(true, queue)?;
            let lease = WatchLease { cancel: Some(lists.cancel_handle()), executor };
            let mut all = Vec::new();
            let mut cursor = None;
            let mut cursors = HashSet::new();
            loop {
                let reply: host::ThreadsPage = client.request(&if queue { Request::ListThreadsQueue { archived: true, limit: 200, cursor } } else { Request::list_threads_page(None, Some(200), cursor, true) })?;
                navigation::merge_threads(&mut all, reply.threads).map_err(host::Error::Protocol)?;
                cursor = reply.next_cursor;
                if cursor.is_none() { break; }
                if !cursors.insert(cursor.clone()) || all.len() >= 20_000 {
                    return Err(host::Error::Protocol("Thread listing exceeded its pagination limit. Refine the host's stored threads.".into()));
                }
            }
            Ok::<_, host::Error>((lists, lease, all, queue))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (mut lists, lease, all, queue) = match task.await {
                Ok(value) => value,
                Err(error) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.thread_list_failed(generation, error, cx)
                    });
                    return;
                }
            };
            let registered = this
                .update_in(cx, |this, window, cx| {
                    if generation != this.navigation.generation {
                        return false;
                    }
                    this.navigation.watch = Some(lease);
                    this.admission.queue = queue;
                    this.navigation.loading = false;
                    this.host.connection.threads = all;
                    this.apply_thread_activity(true, window, cx);
                    this.reconcile_checkout_waits(window, cx);
                    this.rebuild_projects(cx);
                    this.tabs.identity.catalog_ready = true;
                    this.verify_tab_identity(window, cx);
                    this.try_restore_tabs(window, cx);
                    this.restore_thread_selection(window, cx);
                    true
                })
                .unwrap_or(false);
            if !registered {
                cx.background_executor()
                    .spawn(async move {
                        drop(lists);
                    })
                    .detach();
                return;
            }
            while let Some(result) = lists.next().await {
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if generation != this.navigation.generation {
                            return false;
                        }
                        match result {
                            Ok(live) => {
                                let (relabel, ended) = this.worktree_summaries(&live);
                                let review_changed = live.iter().any(|new| {
                                    new.checkout == this.host.creation.checkout
                                        && this
                                            .host
                                            .connection
                                            .threads
                                            .iter()
                                            .find(|old| old.id == new.id)
                                            .is_none_or(|old| old.status != new.status)
                                });
                                if let Err(error) = navigation::merge_threads(
                                    &mut this.host.connection.threads,
                                    live,
                                ) {
                                    this.thread_list_failed(
                                        generation,
                                        host::Error::Protocol(error),
                                        cx,
                                    );
                                    return false;
                                }
                                if review_changed {
                                    this.refresh_turn_review(cx);
                                }
                                this.worktrees_after_summaries(relabel, ended, cx);
                                this.apply_thread_activity(false, window, cx);
                                this.reconcile_checkout_waits(window, cx);
                                this.rebuild_projects(cx);
                                this.capture_session(cx);
                                cx.notify();
                                true
                            }
                            Err(error) => {
                                this.thread_list_failed(generation, error, cx);
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            cx.background_executor()
                .spawn(async move {
                    drop(lists);
                })
                .detach();
        })
        .detach();
    }

    fn thread_list_failed(&mut self, generation: u64, error: host::Error, cx: &mut Context<Self>) {
        if generation != self.navigation.generation {
            return;
        }
        self.navigation.watch = None;
        self.navigation.loading = false;
        self.navigation.error = Some(format!("Thread updates paused: {error}"));
        cx.notify();
    }

    fn apply_thread_activity(
        &mut self,
        initial: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preferences.loaded {
            return;
        }
        let visible = (window.is_window_active()
            && !self.settings_open
            && self.workspace.tab == super::Workspace::Conversation
            && self.conversation_snapshot().is_some())
        .then_some(self.host.thread.selected.as_deref())
        .flatten();
        let mut preferences = self.preferences.value.clone();
        let notifications =
            preferences
                .navigation
                .observe(&self.host.connection.threads, visible, initial);
        if preferences != self.preferences.value {
            self.persist_navigation(preferences, cx);
        }
        if cfg!(test) {
            return;
        }
        // In front, an alert is a toast; in the background, a system notification.
        let active = window.is_window_active();
        for id in notifications {
            let Some(thread) = self
                .host
                .connection
                .threads
                .iter()
                .find(|thread| thread.id == id)
            else {
                continue;
            };
            let title = self.thread_title(&id);
            let detail = format!(
                "{} · {}",
                plait_core::state::project_name(&thread.checkout),
                navigation::state_label(thread)
            );
            if active {
                self.show_toast(&id, title, detail, cx);
            } else {
                window.request_attention();
                cx.show_system_notification(SystemNotification {
                    tag: format!("plait-thread:{id}").into(),
                    title: title.into(),
                    body: detail.into(),
                    actions: Vec::new(),
                });
            }
        }
    }

    /// Git resolves subdirectories and symlinks to the same checkout root as
    /// the host. Only an accepted repository result reaches this method.
    pub(super) fn normalize_checkout(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self
            .state
            .repository
            .snapshot()
            .ready()
            .map(|snapshot| snapshot.root.clone())
        else {
            return;
        };
        if root == self.host.creation.checkout {
            return;
        }
        let previous = std::mem::replace(&mut self.host.creation.checkout, root.clone());
        self.state.repository.data.path = root.clone();
        let mut preferences = self.preferences.value.clone();
        preferences
            .navigation
            .opened
            .retain(|path| path != &previous);
        preferences.navigation.open(&root);
        self.persist_navigation(preferences, cx);
        self.rebuild_projects(cx);
        self.capture_session(cx);
    }

    pub(super) fn rebuild_projects(&mut self, cx: &mut Context<Self>) {
        let machine = self.state.projects[0].machines[0].name.clone();
        self.state.projects = navigation::projects(
            &self.host.connection.threads,
            &self.preferences.value.navigation,
            &self.host.creation.checkout,
            &machine,
        );
        self.refresh_tab_rows();
        let paths = self
            .state
            .projects
            .iter()
            .map(|project| project.checkout.clone())
            .collect::<Vec<_>>();
        let pending = paths
            .into_iter()
            .filter(|path| !self.navigation.available.contains_key(path))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return;
        }
        let generation = self.navigation.generation;
        let task = cx.background_executor().spawn(async move {
            pending
                .into_iter()
                .map(|path| {
                    let available = path.is_dir();
                    (path, available)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.navigation.generation {
                    return;
                }
                this.navigation.available.extend(results);
                this.sync_composer(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_managed_thread(
        &mut self,
        summary: ThreadSummary,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        navigation::merge_threads(&mut self.host.connection.threads, vec![summary])?;
        self.rebuild_projects(cx);
        self.capture_session(cx);
        cx.notify();
        Ok(())
    }
}
