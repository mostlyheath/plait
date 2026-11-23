//! Worktree effects belong to the host; the window only starts and observes them.
use super::Plait;
use gpui::*;
use plait_client::{
    host::{
        self, Client, ThreadSummary,
        worktree::{self, Operation, Request, Stage},
    },
    host_state::request_id,
    worktree_removal,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Pending {
    request: Request,
    /// The checkout the person asked from; a retry elsewhere is a new request.
    checkout: PathBuf,
    selection: u64,
    tab: Option<plait_client::tabs::TabId>,
    connection: u64,
    host: plait_client::tabs::HostKey,
}
#[cfg(test)]
impl Pending {
    pub(super) fn for_test(checkout: PathBuf, host: plait_client::tabs::HostKey) -> Self {
        Self {
            request: Request::ListWorktrees,
            checkout,
            selection: 0,
            tab: None,
            connection: 0,
            host,
        }
    }
}
const BUSY: &str =
    "Another worktree is still being created or removed. Try again when it finishes.";
impl Plait {
    pub(super) fn new_worktree_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(true, window, cx);
        if self.worktree_busy {
            self.notice = Some(BUSY.into());
            return cx.notify();
        }
        if let Some(mut pending) = self.worktree_pending.clone() {
            if pending.host != self.tabs.host {
                self.notice = Some(
                    "Reconnect to the original host before retrying this worktree request.".into(),
                );
                return cx.notify();
            }
            if pending.checkout == self.visible_project_checkout() {
                pending.connection = self.host.connection.epoch;
                return self.send_worktree(pending, window, cx);
            }
            // Another project gets its own request. The host keeps the earlier
            // receipt, and reconnecting lists it.
            self.worktree_pending = None;
        }
        if !self.host.connection.connected {
            self.notice = Some("Connect to the host to start a thread in a worktree.".into());
            return cx.notify();
        }
        // Respect the selected draft, or the actual configured default. Never
        // fall back to Pi just because a chosen harness is unavailable.
        let harness = if self.draft_visible() {
            self.draft
                .harness
                .unwrap_or(self.preferences.value.default_harness)
        } else {
            self.preferences.value.default_harness
        };
        let choice = if self.draft_visible() {
            self.draft
                .model
                .clone()
                .or_else(|| self.default_model_for(harness))
        } else {
            self.default_model_for(harness)
        };
        let Some(choice) = choice.filter(|choice| self.usable_in(harness, choice)) else {
            self.notice = Some(format!(
                "Choose an available {} model before creating a worktree.",
                harness.label()
            ));
            return cx.notify();
        };
        let mode = if self.draft_visible() {
            self.draft_mode()
        } else {
            self.harness_row(harness)
                .and_then(|row| row.default_mode.clone())
        };
        let thinking = if self.draft_visible() {
            self.draft_thinking()
        } else {
            Some(self.preferences.value.default_thinking())
        };
        let approval = if self.draft_visible() {
            self.draft_approval()
        } else {
            self.default_approval()
        };
        let root = self.visible_project_checkout().to_owned();
        let checkout = root.clone();
        let selection = self.host.thread.watch_epoch;
        let tab = self.tabs.model.active();
        let connection = self.host.connection.epoch;
        self.worktree_busy = true;
        self.notice = Some("Creating a worktree from the selected commit…".into());
        cx.notify();
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { worktree::info(&client, root) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.worktree_busy = false;
                if this.host.connection.epoch != connection {
                    this.notice = Some("The host connection changed before the worktree was requested. Nothing was created; try again.".into());
                    return cx.notify();
                }
                match result {
                    Ok(info) => {
                        let pending = Pending {
                            checkout,
                            request: Request::CreateWorktreeThread {
                                request_id: request_id(),
                                parent: Box::new(info.parent),
                                harness,
                                provider: choice.provider,
                                model_id: choice.model_id,
                                thinking,
                                mode,
                                approval,
                            },
                            selection,
                            tab,
                            connection,
                            host: this.tabs.host.clone(),
                        };
                        this.worktree_pending = Some(pending.clone());
                        this.send_worktree(pending, window, cx);
                    }
                    Err(error) => {
                        this.notice = Some(format!("Worktree not created. {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn send_worktree(&mut self, pending: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let build_generation =
            if let Request::CreateWorktreeThread { request_id, .. } = &pending.request {
                Some(self.watch_build_output(
                    request_id.clone(),
                    host::build_output::Kind::Worktree,
                    cx,
                ))
            } else {
                None
            };
        self.worktree_busy = true;
        self.notice = Some("Creating worktree… You can keep working in another tab.".into());
        cx.notify();
        let request = pending.request.clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {

            let operation = await_operation(&client, &request)?;
            let summary = if operation.state == Stage::Ready {
                let reply: host::ThreadReply = client.request(&host::Request::Watch {
                    thread_id: operation.thread_id.clone(),
                    protocol_version: Some(host::PROTOCOL_VERSION),
                })?;
                let tree = operation.worktree.as_ref().ok_or_else(|| host::Error::Protocol("Host omitted the created worktree.".into()))?;
                let matches_choice = match &request {
                    Request::CreateWorktreeThread { harness, provider, model_id, .. } =>
                        super::harnesses::harness_of(&reply.thread) == *harness
                            && reply.thread.model.provider == *provider && reply.thread.model.model_id == *model_id,
                    _ => false,
                };
                if reply.thread.id != operation.thread_id || reply.thread.checkout != tree.path || !matches_choice {
                    return Err(host::Error::Protocol("Host returned a different thread, checkout, or harness. Creation remains unconfirmed.".into()));
                }
                Some(ThreadSummary::from(&reply.thread))
            } else {
                None
            };
            Ok::<_, host::Error>((operation, summary))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.worktree_busy = false;
                if this.host.connection.epoch != pending.connection {
                    this.notice = Some("The host connection changed before the worktree reply arrived. Creation is unconfirmed; use New Thread in Worktree again to confirm it.".into());
                    return cx.notify();
                }
                if let Request::CreateWorktreeThread { request_id, .. } = &pending.request && let Some(generation) = build_generation { this.finish_build_watch(request_id, generation); }
                match result {
                    Ok((operation, summary)) => {
                        this.worktree_pending = None;
                        this.remember_worktree_operation(&operation, cx);
                        if let Some(summary) = summary {
                            let id = summary.id.clone();
                            let _ = plait_client::navigation::merge_threads(this.host.connection.threads_mut(), vec![summary]);
                            this.rebuild_projects(cx);
                            if completion_visible(pending.selection, this.host.thread.watch_epoch, pending.tab, this.tabs.model.active()) {
                                this.notice = None; this.open_created_thread(id, window, cx);
                            } else { this.notice = Some("Worktree thread created in the background. Open it from the sidebar.".into()); }
                        } else { this.notice = Some(operation.error.map(|error| error.message).unwrap_or_else(|| "Worktree creation needs inspection. Existing files and branch were preserved.".into())); }
                    }
                    Err(error) => { this.notice = Some(format!("{error} Creation is unconfirmed. Use New Thread in Worktree again to retry the same request.")); }
                }
                cx.notify();
            });
        }).detach();
    }
    pub(super) fn remember_worktree_operation(
        &mut self,
        operation: &Operation,
        cx: &mut Context<Self>,
    ) {
        self.update_lists(|lists| lists.remember_host_worktree(operation), cx);
    }
    /// Reconcile receipts after a closed window or a lost acknowledgement.
    pub(super) fn refresh_host_worktrees(&mut self, cx: &mut Context<Self>) {
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { worktree::list(&client) });
        cx.spawn(async move |this, cx| {
            if let Ok(operations) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.host.connection.epoch != epoch {
                        return;
                    }
                    this.update_lists(
                        |lists| {
                            for operation in &operations {
                                lists.remember_host_worktree(operation);
                            }
                        },
                        cx,
                    );
                    this.rebuild_projects(cx);
                });
            }
        })
        .detach();
    }
    pub(super) fn remove_worktree(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        if self.worktree_busy {
            self.navigation.error = Some(BUSY.into());
            return cx.notify();
        }
        if let Some(pending) = &self.worktrees.removal.pending {
            if pending.path == path {
                self.finish_worktree_removal(cx);
            } else {
                self.navigation.error = Some(format!(
                    "Another worktree's removal is unconfirmed. {}",
                    worktree_removal::RECOVERY_HINT
                ));
                cx.notify();
            }
            return;
        }
        let Some(tree) = self.lists().worktree(&path).cloned() else {
            return;
        };
        if tree.creation_id.is_none() || tree.identity.is_none() {
            self.navigation.error = Some("This is an imported worktree. Remove it with your Git tools; Plait has no host ownership record.".into());
            return cx.notify();
        }
        if !self.worktrees.removal.ask_record(
            path.clone(),
            &tree,
            self.tabs.host.clone(),
            request_id(),
        ) {
            return;
        }
        let detail = worktree_removal::confirmation_detail(&path, &tree.branch, None);
        self.show_worktree_removal(tree.main, detail, window, cx);
    }
    /// Both entry points use the same frozen choices, request and reply ledger.
    pub(super) fn show_worktree_removal(
        &mut self,
        project: PathBuf,
        detail: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(asked) = self.worktrees.removal.asking.clone() else {
            return;
        };
        let choices = self.worktrees.removal.choices().offered();
        let mut buttons: Vec<_> = choices
            .iter()
            .map(|choice| PromptButton::Other(choice.label().into()))
            .collect();
        buttons.push(PromptButton::Cancel("Cancel".into()));
        self.worktrees.removal_project = Some(project);
        let answer = window.prompt(
            PromptLevel::Warning,
            "Remove this worktree?",
            Some(&detail),
            &buttons,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let choice = answer
                .await
                .ok()
                .and_then(|index| choices.get(index).copied());
            let _ = this.update(cx, |this, cx| {
                if this.worktrees.removal.asking.as_ref() != Some(&asked) {
                    return;
                }
                if this.worktrees.removal.answer(choice).is_some() {
                    this.finish_worktree_removal(cx);
                }
            });
        })
        .detach();
    }
    pub(super) fn finish_worktree_removal(&mut self, cx: &mut Context<Self>) {
        if self.worktree_busy {
            self.worktrees.removal.message = Some(format!(
                "Another worktree operation is in progress. {}",
                worktree_removal::RECOVERY_HINT
            ));
            return cx.notify();
        }
        let Some(attempt) = self.worktrees.removal.pending.clone() else {
            return;
        };
        if attempt.host != self.tabs.host {
            self.worktrees.removal.message =
                Some("Reconnect the original host to recover this removal.".into());
            return cx.notify();
        }
        self.worktree_busy = true;
        let request = attempt.request.clone();
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { super::worktree::await_operation(&client, &request) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.worktree_busy = false;
                this.settle_worktree_removal(&attempt, result, cx);
            });
        })
        .detach();
    }
    /// Removal here must leave the sidebar and composer where the sidebar's own
    /// Remove leaves them: the folder is gone, not merely re-listed.
    pub(super) fn settle_worktree_removal(
        &mut self,
        attempt: &plait_client::worktree_removal::Attempt,
        result: Result<worktree::Operation, host::Error>,
        cx: &mut Context<Self>,
    ) {
        let removed = result
            .as_ref()
            .ok()
            .filter(|operation| {
                operation.error.is_none() && operation.state == worktree::Stage::Removed
            })
            .cloned();
        if !self
            .worktrees
            .removal
            .finish(attempt, result.map_err(|e| e.to_string()))
        {
            return;
        }
        if !self.worktrees_visible()
            && attempt.host == self.tabs.host
            && self.worktrees.removal_project.as_deref()
                == Some(self.project_of(self.visible_project_checkout()))
        {
            self.notice = self.worktrees.removal.message.clone();
        }
        if let Some(operation) = removed {
            self.remember_worktree_operation(&operation, cx);
            self.navigation
                .available
                .insert(attempt.path.clone(), false);
            self.sync_composer(cx);
        }
        self.refresh_host_worktrees(cx);
        self.refresh_worktrees_view(false, cx);
        cx.notify();
    }
}
fn completion_visible<T: PartialEq>(
    selection: u64,
    current: u64,
    tab: Option<T>,
    active: Option<T>,
) -> bool {
    selection == current && tab.is_some() && tab == active
}
pub(super) fn await_operation(
    client: &Client,
    request: &Request,
) -> Result<Operation, host::Error> {
    let expected = match request {
        Request::CreateWorktreeThread { request_id, .. }
        | Request::RemoveWorktree { request_id, .. } => request_id.clone(),
        _ => return Err(host::Error::Protocol("Invalid worktree mutation.".into())),
    };
    let mut operation = worktree::operation(client, request)?;
    let deadline = Instant::now() + Duration::from_secs(330);
    while matches!(operation.state, Stage::Preparing | Stage::Removing) {
        if Instant::now() >= deadline {
            return Err(host::Error::Protocol(
                "Worktree is still preparing on the host.".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
        operation = worktree::operation(
            client,
            &Request::WorktreeStatus {
                request_id: expected.clone(),
            },
        )?;
    }
    if let Request::CreateWorktreeThread {
        request_id, parent, ..
    } = request
        && (operation.request_id != *request_id
            || operation
                .worktree
                .as_ref()
                .is_some_and(|tree| tree.parent != **parent))
    {
        return Err(host::Error::Protocol(
            "Host returned a different worktree operation.".into(),
        ));
    }
    if let Request::RemoveWorktree { creation_id, .. } = request
        && operation.request_id != *creation_id
    {
        return Err(host::Error::Protocol(
            "Host returned a different removal operation.".into(),
        ));
    }
    Ok(operation)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[core::prelude::v1::test]
    fn completion_after_switch_or_close_never_steals_focus() {
        assert!(completion_visible(7, 7, Some(1), Some(1)));
        assert!(!completion_visible(7, 8, Some(1), Some(1)));
        assert!(!completion_visible(7, 7, Some(1), Some(2)));
        assert!(!completion_visible(7, 7, Some(1), None));
        assert!(!completion_visible::<u32>(7, 7, None, None));
    }
}
