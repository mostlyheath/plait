//! Draft identities own configuration and creation, independently of selection.
use super::{
    Plait,
    host::Created,
    new_thread::{Draft, Sending},
};
use gpui::*;
use plait_client::{
    host::{
        self,
        admission::{self, Placement, Request, Stage},
        harness::HarnessId,
    },
    session::{DraftSettings, DraftTabCreation, Thread},
    tabs::TabTarget,
};

pub(super) struct Pending {
    pub request: Request,
    pub creating: bool,
    pub choice: bool,
}

impl Draft {
    pub(super) fn settings(&self) -> DraftSettings {
        DraftSettings {
            harness: self.harness,
            model: self.model.clone(),
            mode: self.mode.clone(),
            thinking: self.thinking,
            approval: self.approval,
        }
    }
    pub(super) fn legacy_creation(&self) -> Option<plait_client::session::DraftCreation> {
        let pending = self.pending.as_ref()?;
        let sending = self.sending.as_ref()?;
        if pending.choice {
            return None;
        }
        matches!(&pending.request, Request::CreateAdmittedThread { request_id, .. } if request_id == &sending.request_id)
            .then(|| {
                let mut request = pending.request.clone();
                let mut wait = false;
                if let Request::CreateAdmittedThread { placement, .. } = &mut request {
                    (*placement, wait) = placement.journaled();
                }
                plait_client::session::DraftCreation {
                    request, approval: sending.approval, choice: sending.choice, wait,
                }
            })
    }

    pub(super) fn saved_creation(&self) -> Option<DraftTabCreation> {
        let pending = self.pending.as_ref()?;
        let sending = self.sending.as_ref()?;
        let Request::CreateAdmittedThread {
            request_id,
            placement,
            harness,
            provider,
            model_id,
            mode,
            thinking,
            approval,
            ..
        } = &pending.request
        else {
            return None;
        };
        let (placement, wait) = placement.journaled();
        Some(DraftTabCreation {
            request_id: request_id.clone(),
            placement,
            wait,
            harness: *harness,
            model: host::ModelChoice {
                provider: provider.clone(),
                model_id: model_id.clone(),
            },
            mode: mode.clone(),
            thinking: *thinking,
            approval: *approval,
            message_approval: sending.approval,
            thread_approval: sending.choice,
            message: plait_client::session::Draft {
                text: sending.text.clone(),
                revision: sending.revision,
                images: sending.images.clone(),
                submitted: None,
                creation: None,
            },
            needs_choice: pending.choice,
        })
    }

    fn restore_creation(&mut self, saved: DraftTabCreation, checkout: std::path::PathBuf) {
        self.harness.get_or_insert(saved.harness);
        self.model.get_or_insert(saved.model.clone());
        self.mode = self.mode.clone().or(saved.mode.clone());
        self.thinking = self.thinking.or(saved.thinking);
        self.approval.get_or_insert(saved.approval);
        self.pending = Some(Pending {
            request: Request::CreateAdmittedThread {
                request_id: saved.request_id.clone(),
                checkout,
                placement: saved.placement.from_journal(saved.wait),
                harness: saved.harness,
                provider: saved.model.provider,
                model_id: saved.model.model_id,
                approval: saved.approval,
                mode: saved.mode,
                thinking: saved.thinking,
                source_thread_id: None,
            },
            creating: false,
            choice: saved.needs_choice,
        });
        self.sending = Some(Sending {
            generation: self.generation,
            request_id: saved.request_id,
            text: saved.message.text,
            images: saved.message.images,
            revision: saved.message.revision,
            approval: saved.message_approval,
            choice: saved.thread_approval,
        });
    }
}

impl Plait {
    pub(super) fn park_draft(&mut self, cx: &mut Context<Self>) {
        self.refresh_draft_title(cx);
        let mut draft = std::mem::take(&mut self.draft);
        if let Some(composer) = draft.composer.take() {
            let composer = composer.read(cx);
            let (text, revision, submitted) = composer.saved_draft();
            draft.saved = plait_client::session::Draft {
                text: text.into(),
                revision,
                submitted: submitted.clone(),
                images: composer.saved_images().to_vec(),
                creation: draft.legacy_creation(),
            };
        }
        draft.open = false;
        if let Some(id) = draft.id.clone() {
            self.tabs.lifecycle.drafts.insert(id, draft);
        }
    }

    pub(super) fn restore_draft_tab(&mut self, id: String, cx: &mut Context<Self>) {
        if self.draft.id.as_ref() == Some(&id) {
            return;
        }
        self.park_draft(cx);
        self.draft = self.tabs.lifecycle.drafts.remove(&id).unwrap_or_else(|| {
            let saved = self
                .session
                .data
                .as_ref()
                .and_then(|s| s.draft_tab(&id, &self.host.creation.checkout))
                .cloned();
            let settings = saved
                .as_ref()
                .map(|tab| tab.settings.clone())
                .unwrap_or_default();
            let mut draft = Draft {
                id: Some(id),
                harness: settings.harness,
                model: settings.model,
                mode: settings.mode,
                thinking: settings.thinking,
                approval: settings.approval,
                ..Default::default()
            };
            if let Some(tab) = saved
                && let Some(creation) = tab
                    .creation
                    .or_else(|| DraftTabCreation::from_legacy(&tab.draft, &tab.checkout))
            {
                draft.restore_creation(creation, tab.checkout);
            }
            draft
        });
    }

    pub(super) fn draft_isolation_choice(&self) -> bool {
        self.draft.pending.as_ref().is_some_and(|p| p.choice)
            || (self.draft.pending.is_none() && self.admission.choice)
    }

    pub(super) fn send_draft_tab(
        &mut self,
        text: String,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.draft_creating() || !self.host.connection.connected || !self.session.ready() {
            return;
        }
        if let Some(reason) = self.draft_blocked() {
            self.draft.error = Some(reason);
            cx.notify();
            return;
        }
        if self.draft.pending.is_none() {
            let Some(choice) = self.draft_model() else {
                return;
            };
            let request_id = plait_client::host_state::request_id();
            let harness = self.draft_harness();
            let approval = self.draft_approval();
            self.draft.pending = Some(Pending {
                request: Request::CreateAdmittedThread {
                    request_id: request_id.clone(),
                    checkout: self.host.creation.checkout.clone(),
                    placement: Placement::Project,
                    harness,
                    provider: choice.provider,
                    model_id: choice.model_id,
                    approval,
                    mode: self.draft_mode(),
                    thinking: self.draft_thinking(),
                    source_thread_id: None,
                },
                creating: false,
                choice: false,
            });
            self.draft.sending = Some(Sending {
                generation: self.draft.generation,
                request_id,
                text,
                revision,
                images: self
                    .draft
                    .composer
                    .as_ref()
                    .map(|c| c.read(cx).saved_images().to_vec())
                    .unwrap_or_default(),
                approval: (harness == HarnessId::Pi && self.host.protocol() >= 2)
                    .then_some(approval),
                choice: self.draft.approval.filter(|_| harness == HarnessId::Pi),
            });
        }
        self.start_draft_creation(window, cx);
    }

    pub(super) fn start_draft_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host.connection.connected || !self.session.ready() {
            return;
        }
        let Some(pending) = self.draft.pending.as_mut() else {
            return;
        };
        if pending.creating {
            return;
        }
        let Request::CreateAdmittedThread {
            request_id,
            checkout,
            harness,
            provider,
            model_id,
            mode,
            thinking,
            ..
        } = &pending.request
        else {
            return;
        };
        let choice = host::ModelChoice {
            provider: provider.clone(),
            model_id: model_id.clone(),
        };
        let created = Created {
            draft_id: self.draft.id.clone(),
            request_id: request_id.clone(),
            choice: choice.clone(),
            harness: *harness,
            selection_epoch: self.host.thread.watch_epoch,
            checkout: Some(checkout.clone()),
        };
        let legacy = host::Request::create_thread(
            checkout.clone(),
            &choice,
            request_id.clone(),
            *thinking,
            *harness,
            self.host.connection.protocol(),
        )
        .with_mode(mode.clone());
        pending.creating = true;
        pending.choice = false;
        self.draft.error = None;
        self.capture_session(cx);
        self.create_with_admission(created, legacy, window, cx);
        self.sync_composer(cx);
        cx.notify();
    }

    pub(super) fn accept_draft_isolation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_draft_placement(Placement::Worktree, window, cx);
    }
    pub(super) fn accept_draft_wait(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.admission.queue {
            self.choose_draft_placement(Placement::Wait, window, cx);
        }
    }
    fn choose_draft_placement(
        &mut self,
        chosen: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .draft
            .pending
            .as_ref()
            .is_some_and(|pending| pending.choice)
        {
            return;
        }
        let Some(composer) = &self.draft.composer else {
            return;
        };
        let composer = composer.read(cx);
        if composer.is_empty() {
            self.notice =
                Some("Write a message or attach an image before choosing Own worktree.".into());
            cx.notify();
            return;
        }
        // Nothing has started at the choice. Capture this draft's latest edit now.
        let (text, revision, _) = composer.saved_draft();
        if let Some(sending) = &mut self.draft.sending {
            sending.text = text.into();
            sending.revision = revision;
            sending.images = composer.saved_images().to_vec();
        }
        if let Some(Pending {
            request: Request::CreateAdmittedThread { placement, .. },
            ..
        }) = &mut self.draft.pending
        {
            *placement = chosen;
        }
        self.start_draft_creation(window, cx);
    }

    pub(super) fn cancel_draft_isolation(&mut self, cx: &mut Context<Self>) {
        if !self.draft_isolation_choice() {
            return;
        }
        self.draft.pending = None;
        self.draft_rejected("Cancelled. Your draft is kept.".into());
        self.capture_session(cx);
        self.sync_composer(cx);
        cx.notify();
    }

    pub(super) fn finish_draft_creation(
        &mut self,
        created: Created,
        epoch: u64,
        result: Result<(Option<admission::Admission>, Option<host::ThreadReply>), host::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.connection.epoch != epoch {
            return;
        }
        let Some(id) = &created.draft_id else {
            return;
        };
        let active = self.draft.id.as_ref() == Some(id);
        let owner = if active {
            Some(&self.draft)
        } else {
            self.tabs.lifecycle.drafts.get(id)
        };
        if !owner.is_some_and(|d| {
            d.sending
                .as_ref()
                .is_some_and(|s| s.request_id == created.request_id && s.generation == d.generation)
                && d.pending.is_some()
        }) {
            return;
        }
        let mut draft = if active {
            std::mem::take(&mut self.draft)
        } else {
            self.tabs.lifecycle.drafts.remove(id).unwrap()
        };
        draft.pending.as_mut().unwrap().creating = false;
        let ready = match result {
            Ok((Some(receipt), _)) if receipt.state == Stage::Choice => {
                draft.pending.as_mut().unwrap().choice = true;
                None
            }
            Ok((receipt, Some(reply))) => {
                let checkout = receipt
                    .as_ref()
                    .map(|r| &r.checkout)
                    .or(created.checkout.as_ref());
                if checkout.is_some_and(|root| root != &reply.thread.checkout)
                    || reply.thread.model != created.choice
                    || super::harnesses::harness_of(&reply.thread) != created.harness
                {
                    draft.error = Some("Host returned a different checkout, model or harness. Retry creation safely.".into());
                    None
                } else {
                    Some((receipt, reply))
                }
            }
            Ok((receipt, None)) => {
                let error = receipt
                    .and_then(|r| r.error)
                    .map(host::Error::Rejected)
                    .unwrap_or_else(|| {
                        host::Error::Protocol(
                            "Creation is still preparing. Retry with the same request.".into(),
                        )
                    });
                if super::host::is_checkout_identity_changed(&error) {
                    let path = created
                        .checkout
                        .clone()
                        .unwrap_or_else(|| self.host.creation.checkout.clone());
                    draft.error = Some(Self::identity_message(&path));
                    self.tabs.identity.recovery = Some((path, false));
                }
                fail(&mut draft, error);
                None
            }
            Err(error) => {
                if super::host::is_checkout_identity_changed(&error) {
                    let path = created
                        .checkout
                        .clone()
                        .unwrap_or_else(|| self.host.creation.checkout.clone());
                    draft.error = Some(Self::identity_message(&path));
                    self.tabs.identity.recovery = Some((path, false));
                }
                fail(&mut draft, error);
                None
            }
        };
        if let Some((receipt, reply)) = ready {
            self.deliver_draft_tab(created, draft, active, (receipt, reply), window, cx);
        } else if active {
            self.draft = draft;
        } else {
            self.tabs.lifecycle.drafts.insert(id.clone(), draft);
        }
        self.capture_draft_operations(cx);
        self.sync_composer(cx);
        cx.notify();
    }

    fn capture_draft_operations(&mut self, cx: &mut Context<Self>) {
        if let Some(data) = &mut self.session.data {
            let mut mirrors = Vec::new();
            for draft in std::iter::once(&self.draft).chain(self.tabs.lifecycle.drafts.values()) {
                if let Some(id) = &draft.id
                    && let Some(tab) = data.draft_tabs.get_mut(id)
                {
                    tab.creation = draft.saved_creation();
                    tab.draft.creation = draft.legacy_creation();
                    if tab.legacy {
                        mirrors.push((tab.checkout.clone(), tab.draft.clone()));
                    }
                }
            }
            for (checkout, draft) in mirrors {
                data.set_new_draft(checkout, draft);
            }
            if let Some(journal) = &self.session.journal {
                journal.queue(data.clone());
            }
        }
        self.capture_session(cx);
        self.schedule_session_save(false, cx);
    }

    fn deliver_draft_tab(
        &mut self,
        created: Created,
        mut draft: Draft,
        active: bool,
        ready: (Option<admission::Admission>, host::ThreadReply),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let checkout_wait = draft.pending.as_ref().is_some_and(|pending| {
            matches!(
                pending.request,
                Request::CreateAdmittedThread {
                    placement: Placement::Wait,
                    ..
                }
            )
        });
        let (receipt, reply) = ready;
        let draft_id = created.draft_id.unwrap();
        let view = self.workspace.tab;
        let in_front = active && self.tabs.model.active().and_then(|tab| self.tabs.model.get(tab)).is_some_and(|tab| matches!(tab.session().target(), TabTarget::Draft { id, .. } if id == &draft_id));
        let id = reply.thread.id.clone();
        let source_root = created.checkout.unwrap();
        let source = self.tab_checkout(source_root.clone());
        if let Some(tree) = receipt.and_then(|r| r.worktree) {
            if let Some(identity) = tree.identity {
                self.tabs.identity.checkouts.insert(
                    tree.path.clone(),
                    plait_client::tabs::DirectoryIdentity {
                        device: identity.dev.parse().unwrap(),
                        inode: identity.ino.parse().unwrap(),
                    },
                );
            }
            self.notice = Some(format!(
                "Worktree: {} · {}. Starts from HEAD; original uncommitted files and build output were not copied.",
                tree.path.display(),
                tree.branch
            ));
            self.refresh_host_worktrees(cx);
        }
        if !self.host.connection.threads.iter().any(|t| t.id == id) {
            self.host
                .connection
                .threads
                .insert(0, host::ThreadSummary::from(&reply.thread));
            self.rebuild_projects(cx);
        }
        let target = TabTarget::Existing {
            id: id.clone(),
            checkout: self.tab_checkout(reply.thread.checkout.clone()),
        };
        let draft_target = TabTarget::Draft {
            id: draft_id.clone(),
            checkout: source,
        };
        let result = if self
            .session
            .data
            .as_ref()
            .is_some_and(|data| data.threads.contains_key(&id))
        {
            Err(plait_client::tabs::TabError::AlreadyOpen)
        } else {
            self.tabs.model.bind_draft_target(&draft_target, target)
        };
        if let Err(error) = result {
            draft.error = Some(format!(
                "Thread creation could not be attached: {error} Your draft is kept."
            ));
            if active {
                self.draft = draft;
            } else {
                self.tabs.lifecycle.drafts.insert(draft_id, draft);
            }
            return;
        }
        let Some(sending) = draft.sending.take() else {
            return;
        };
        if let Some(data) = &mut self.session.data {
            let mut saved = data
                .draft_tab(&draft_id, &source_root)
                .map(|tab| tab.draft.clone())
                .unwrap_or_else(|| plait_client::session::Draft {
                    text: sending.text.clone(),
                    images: sending.images.clone(),
                    revision: sending.revision,
                    submitted: None,
                    creation: None,
                });
            saved.creation = None;
            // Identity conflicts were rejected before the tab was rebound.
            data.threads.insert(
                id.clone(),
                Thread {
                    checkout: reply.thread.checkout.clone(),
                    model: reply.thread.model.clone(),
                    title: plait_client::session::title(&sending.text),
                    draft: saved,
                    pending: None,
                    pending_submissions: Vec::new(),
                    queued_submissions: Vec::new(),
                    approval: sending.choice,
                },
            );
            data.clear_draft_tab(&draft_id);
        }
        if in_front {
            if let Some(composer) = draft.composer.take() {
                self.attach_composer(id.clone(), composer, window, cx);
            }
            self.open_created_thread(id.clone(), window, cx);
            if view != super::Workspace::Conversation {
                self.show_tab(view, window, cx);
            }
        }
        let mode = (self.host.protocol() >= 3).then_some(host::QueueMode::FollowUp);
        match self.host.submissions.begin_images(
            &id,
            sending.text,
            sending.approval,
            mode,
            Some(sending.revision),
            sending.images,
        ) {
            Ok(pending) => {
                if checkout_wait {
                    self.host
                        .submissions
                        .wait_for_checkout(&id, &pending.request_id);
                }
                self.retry_submission_id(&id, &pending.request_id, window, cx);
            }
            Err(error) => self.set_thread_notice(id.clone(), error, true),
        }
        self.record_recent_model(&created.choice, cx);
        self.refresh_tab_rows();
        self.capture_session(cx);
        self.load_threads(window, cx);
    }
}

fn fail(draft: &mut Draft, error: host::Error) {
    if super::host::is_checkout_identity_changed(&error) {
        draft.pending = None;
        draft.sending = None;
    } else if matches!(error, host::Error::Rejected(_)) {
        draft.pending = None;
        draft.sending = None;
        draft.error = Some(format!("Thread not created. {error}"));
    } else {
        draft.error = Some(format!(
            "{error} Retry creation to reuse the same request ID."
        ));
    }
}
