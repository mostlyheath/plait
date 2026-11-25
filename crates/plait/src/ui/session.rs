//! Desktop recovery and save scheduling. Host execution remains independent.

use super::{Plait, composer::Composer};
use gpui::*;
use plait_client::{
    host::{MessageRole, ThreadSummary},
    session::{self, Access, Draft, Journal, Owner, Session, Thread},
};
use std::{path::PathBuf, sync::Arc, time::Duration};

/// What a thread is called before it has a first message or a chosen name.
pub(super) const UNTITLED: &str = "New thread";

#[derive(Default)]
pub(super) struct SessionState {
    pub required: bool,
    pub loading: bool,
    pub closing: bool,
    pub journal: Option<Arc<Journal>>,
    pub data: Option<Session>,
    pub note: Option<String>,
    pub(super) restore_thread: Option<String>,
    save: Option<Task<()>>,
    /// Listed threads whose saved sends are in memory. Only these are
    /// reconciled with the host and written back.
    adopted: std::collections::HashSet<String>,
    /// The verified owner whose composers were last refilled.
    refilled: Option<(
        plait_client::tabs::HostKey,
        plait_client::machines::MachineId,
    )>,
}

impl SessionState {
    pub fn ready(&self) -> bool {
        !self.closing && (!self.required || (!self.loading && self.journal.is_some()))
    }
}

/// A saved draft's verified host and provisional machine.
type DraftOwner = (
    Option<plait_client::tabs::HostKey>,
    Option<plait_client::machines::MachineId>,
);

impl Plait {
    pub(super) fn acknowledge_saved_draft(
        &mut self,
        id: &str,
        text: &str,
        revision: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        // Only a thread whose saved sends are in memory is written back; another
        // machine's work for the same ID is left alone.
        let owned = self.session.adopted.contains(id)
            && self
                .session
                .data
                .as_ref()
                .and_then(|data| data.threads.get(id))
                .is_some_and(|thread| thread.usable(self.access()));
        if let Some(data) = self.session.data.as_mut() {
            if owned && let Some(thread) = data.threads.get_mut(id) {
                let draft = &mut thread.draft;
                let revision = revision.or_else(|| {
                    draft
                        .submitted
                        .as_ref()
                        .filter(|(s, _)| s == text)
                        .map(|(_, r)| *r)
                });
                if revision == Some(draft.revision) && draft.text == text {
                    draft.text.clear();
                    draft.images.clear();
                    draft.submitted = None;
                    draft.revision += 1;
                }
                thread.pending = self.host.submissions.pending(id).cloned();
                thread.pending_submissions = self
                    .host
                    .submissions
                    .all(id)
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect();
                thread.queued_submissions = self.host.submissions.accepted(id).to_vec();
            }
            if let Some(journal) = &self.session.journal {
                journal.queue(data.clone());
            }
            self.schedule_session_save(false, cx);
        }
    }

    pub fn restore_desktop(
        &mut self,
        requested: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                this.close_desktop(false, window, cx);
                false
            })
            .unwrap_or(true)
        });
        self.load_tab_preferences(window, cx);
        self.session.required = true;
        self.session.loading = true;
        self.session.note = Some("Restoring saved workspace…".into());
        self.sync_composer(cx);
        let checkout_generation = self.host.creation.generation;
        let task = cx.background_executor().spawn(async move {
            let (directory, host) = session::locations()?;
            let (journal, mut data) = Journal::open(&directory, host)?;
            data.import_legacy_drafts();
            let path = requested
                .or_else(|| data.checkout.clone().filter(|path| path.is_dir()))
                .unwrap_or_else(|| PathBuf::from("."));
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|root| root.join(&path))
                    .map_err(|error| format!("Cannot restore checkout: {error}"))?
            };
            Ok::<_, String>((journal, data, path))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.session.loading = false;
                match result {
                    Ok((journal, data, path)) => {
                        // This observer outlives the window. Closing it must flush the latest draft.
                        let quit_journal = journal.clone();
                        let quit_view = cx.weak_entity();
                        let quit_window = window.window_handle();
                        let app: &mut App = cx;
                        app.on_app_quit(move |cx| {
                            let journal = quit_journal.clone();
                            let tabs = quit_window.update(cx, |_, window, cx| {
                                quit_view.update(cx, |this, cx| {
                                    this.capture_session(cx);
                                    this.flush_tab_preferences(Some(window), cx)
                                })
                            }).ok().and_then(Result::ok).flatten();
                            cx.background_executor().spawn(async move {
                                if let Some(tabs) = tabs && let Err(error) = tabs.write() { eprintln!("Plait tabs: {error}"); }
                                if let Err(error) = journal.flush() { eprintln!("Plait workspace: {error}"); }
                            })
                        }).detach();
                        let selected = data.selected.clone();
                        this.session.journal = Some(journal);
                        this.session.data = Some(data);
                        this.session.note = None;
                        for (id, composer) in &this.workspace.composers {
                            this.restore_composer(id, composer, cx);
                        }
                        // A folder chosen while disk state loaded wins over the saved folder.
                        if this.host.creation.generation == checkout_generation
                            && !this.host_workspace.checkout_carried
                        {
                            this.change_checkout(path, window, cx);
                            this.session.restore_thread = selected;
                        }
                        this.adopt_saved_work(cx);
                        this.capture_session(cx);
                        this.verify_tab_identity(window, cx);
                        this.try_restore_tabs(window, cx);
                    }
                    Err(error) => this.session.note = Some(format!("Workspace recovery unavailable: {error} Sending is disabled to protect saved work.")),
                }
                this.sync_composer(cx);
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn close_desktop(
        &mut self,
        quit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.closing {
            return;
        }
        self.capture_session(cx);
        let Some(journal) = self.session.journal.clone() else {
            if quit {
                cx.quit();
            } else {
                window.remove_window();
            }
            return;
        };
        let tabs = self.flush_tab_preferences(Some(window), cx);
        self.session.closing = true;
        self.sync_composer(cx);
        let task = cx.background_executor().spawn(async move {
            journal.flush()?;
            if let Some(tabs) = tabs {
                tabs.write()?;
            }
            Ok::<_, String>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if quit {
                        cx.quit();
                    } else {
                        window.remove_window();
                    }
                }
                Err(error) => {
                    this.session.closing = false;
                    this.session.note = Some(format!(
                        "Window kept open because work could not be saved: {error}"
                    ));
                    this.sync_composer(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn change_checkout(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.lifecycle.presenting
            && self.tabs.model.active().is_some()
            && self.host.creation.checkout != path
        {
            self.open_draft_in(path, window, cx);
            return;
        }
        if self.host.creation.creating || self.host.creation.pending_create.is_some() {
            self.notice =
                Some("Resolve the pending thread creation before changing checkout.".into());
            cx.notify();
            return;
        }
        self.clear_turn_review();
        // A draft belongs to its checkout, including while another project is open.
        let drafting = self.draft_visible();
        self.capture_session(cx);
        if self.host.creation.checkout != path {
            self.draft = super::new_thread::Draft {
                generation: self.draft.generation + 1,
                ..Default::default()
            };
        }
        self.update_lists(|lists| lists.open(&path), cx);
        let live = match self.host.change_checkout(path.clone()) {
            Ok(live) => live,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        self.host_workspace.checkout_carried = false;
        self.workspace.file_viewer.clear_checkout();
        self.workspace.line_pick = None;
        self.workspace.thread_files.reset();
        self.policy = Default::default();
        self.forget_agent();
        self.workspace.reset_for_checkout_change();
        self.session.restore_thread = None;
        self.navigation.managing = None;
        self.navigation.rename = None;
        self.navigation.menu = None;
        self.settings_open = false;
        self.new_thread_open = false;
        if drafting {
            self.open_draft(window, cx);
        }
        self.notice = None;
        self.state.open_checkout(path);
        self.rebuild_projects(cx);
        if !live {
            self.reconnect_host(window, cx);
        }
        self.pump(cx);
        self.capture_session(cx);
    }

    pub(super) fn restore_thread_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.persistence.requested && !self.tabs.persistence.restored {
            return;
        }
        if let Some(id) = self.session.restore_thread.take()
            && self.host.thread.selected.is_none()
            && let Some(summary) = self
                .host
                .connection
                .threads()
                .iter()
                .find(|thread| thread.id == id)
            && self.saved_thread(summary).is_some()
        {
            self.select_host_thread(id, window, cx);
        }
    }

    pub(super) fn saved_thread(&self, summary: &ThreadSummary) -> Option<&Thread> {
        self.session
            .data
            .as_ref()?
            .saved_thread(summary, self.access())
    }

    /// What saved work the window may use now: the verified owner's, or before
    /// verification only work saved provisionally for the selected machine.
    pub(super) fn access(&self) -> Access<'_> {
        self.known_owner()
            .map_or(Access::Provisional(&self.machines.active), Access::Verified)
    }

    /// The verified owner of the work on screen: the host identity read on this
    /// connection, for the machine selected now. `None` while connecting, during
    /// a machine switch, or before the identity is read.
    pub(super) fn known_owner(&self) -> Option<Owner<'_>> {
        (self.tabs.identity.verified
            && self.tab_identity_current()
            && self.tabs.identity.machine.as_ref() == Some(&self.machines.active))
        .then_some(Owner {
            host: &self.tabs.host,
            machine: &self.machines.active,
        })
    }

    /// Bring saved sends into memory for listed threads whose saved work is
    /// usable at the listed path. Another path's or another machine's sends,
    /// and those of threads not listed yet, stay saved and untouched: nothing
    /// reconciles or rewrites them here.
    fn adopt_saved_sends(&mut self) {
        let Some(data) = &self.session.data else {
            return;
        };
        let access = self.access();
        let found: Vec<(String, Thread)> = self
            .host
            .connection
            .threads()
            .iter()
            .filter(|summary| !self.session.adopted.contains(&summary.id))
            .filter_map(|summary| {
                data.saved_thread(summary, access)
                    .map(|thread| (summary.id.clone(), thread.clone()))
            })
            .collect();
        for (id, thread) in found {
            for pending in thread.pending.iter().chain(&thread.pending_submissions) {
                self.host.submissions.restore(id.clone(), pending.clone());
            }
            for pending in &thread.queued_submissions {
                self.host
                    .submissions
                    .restore_accepted(id.clone(), pending.clone());
            }
            self.session.adopted.insert(id);
        }
    }

    /// After a check, bring back usable saved sends, and fill composers that
    /// stayed closed until the owner was verified. Runs after each identity
    /// check and when saved work loads.
    pub(super) fn adopt_saved_work(&mut self, cx: &mut Context<Self>) {
        self.adopt_saved_sends();
        let Some(owner) = self.known_owner() else {
            return;
        };
        let key = (owner.host.clone(), owner.machine.clone());
        if self.session.refilled.as_ref() == Some(&key) {
            return;
        }
        self.session.refilled = Some(key);
        let empty: Vec<_> = self
            .workspace
            .composers
            .iter()
            .filter(|(_, composer)| {
                let (text, _, submitted) = composer.read(cx).saved_draft();
                text.is_empty() && submitted.is_none()
            })
            .map(|(id, composer)| (id.clone(), composer.clone()))
            .collect();
        for (id, composer) in empty {
            self.restore_composer(&id, &composer, cx);
        }
    }

    /// Leaving a machine: its saved sends stay in the journal under its owner,
    /// and none of them is shown, reconciled or saved for the next machine.
    pub(super) fn forget_saved_work(&mut self) {
        self.host.submissions = Default::default();
        self.session.adopted.clear();
        self.session.refilled = None;
    }

    pub(super) fn session_accepts_thread(&self, id: &str) -> bool {
        self.host
            .connection
            .threads()
            .iter()
            .find(|summary| summary.id == id)
            .is_some_and(|summary| {
                self.session
                    .data
                    .as_ref()
                    .is_none_or(|data| data.accepts(summary, self.access()))
            })
    }

    pub(super) fn restore_composer(
        &self,
        id: &str,
        composer: &Entity<Composer>,
        cx: &mut Context<Self>,
    ) {
        if let Some(saved) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .and_then(|summary| self.saved_thread(summary))
        {
            let draft = saved.draft.clone();
            let client = self.host_client();
            composer.update(cx, |composer, cx| {
                composer.restore_draft(draft.text, draft.revision, draft.submitted, cx);
                composer.restore_thread_images_with_client(draft.images, id.to_owned(), client, cx)
            });
        }
    }

    /// The owner fields to save draft `id` with, or `None` when its saved
    /// record is not usable now and must stay as it is. A new draft, or one
    /// typed before verification, is saved provisionally for the selected
    /// machine: never dropped, and never taken for another machine's.
    fn draft_owner(&self, id: &str) -> Option<DraftOwner> {
        let access = self.access();
        match self
            .session
            .data
            .as_ref()
            .and_then(|data| data.draft_tabs.get(id))
        {
            Some(saved) if !saved.usable(access) => None,
            _ => Some(access.stamp()),
        }
    }

    /// Whether a saved draft can return here now: the verified owner's, or
    /// before verification one saved provisionally for the selected machine.
    pub(super) fn draft_is_here(&self, tab: &plait_client::session::DraftTab) -> bool {
        tab.usable(self.access())
    }

    /// Why the open draft cannot be sent here: its saved owner is another machine.
    pub(super) fn draft_elsewhere(&self) -> bool {
        self.draft
            .id
            .as_deref()
            .is_some_and(|id| self.draft_owner(id).is_none())
    }

    pub(super) fn capture_session(&mut self, cx: &mut Context<Self>) {
        self.refresh_draft_title(cx);
        self.queue_tab_save(cx);
        // Sends of newly listed threads are in memory before anything is written.
        self.adopt_saved_sends();
        let Some(data) = &self.session.data else {
            return;
        };
        let mut next = data.clone();
        let owner = self.draft.id.as_deref().and_then(|id| self.draft_owner(id));
        if let Some(composer) = &self.draft.composer
            && let Some(id) = &self.draft.id
            && let Some((host, machine)) = owner.clone()
        {
            let composer = composer.read(cx);
            let (text, revision, submitted) = composer.saved_draft();
            next.set_draft_tab(
                id.clone(),
                plait_client::session::DraftTab {
                    legacy: false,
                    creation: self.draft.saved_creation(),
                    checkout: self.host.creation.checkout.clone(),
                    settings: self.draft.settings(),
                    host,
                    machine,
                    draft: Draft {
                        text: text.into(),
                        revision,
                        submitted: submitted.clone(),
                        images: composer.saved_images().to_vec(),
                        creation: self.draft.legacy_creation(),
                    },
                },
            );
        }
        // An open empty tab still survives restart before its composer is mounted.
        if self.draft.composer.is_none()
            && let Some(id) = &self.draft.id
            && let Some((host, machine)) = owner
            && self.tabs.model.tabs().iter().any(|tab| matches!(tab.session().target(), plait_client::tabs::TabTarget::Draft { id: open, .. } if open == id))
        {
            next.set_draft_tab(id.clone(), plait_client::session::DraftTab {
                legacy: false,
                creation: self.draft.saved_creation(),
                checkout: self.host.creation.checkout.clone(),
                settings: self.draft.settings(),
                draft: self.draft.saved.clone(),
                host,
                machine,
            });
        }
        if self.host_is_local()
            && !self.host_workspace.checkout_carried
            && self.host.creation.checkout.is_absolute()
        {
            next.checkout = Some(self.host.creation.checkout.clone());
        }
        next.selected = self
            .host
            .thread
            .selected
            .clone()
            .or_else(|| self.session.restore_thread.clone());
        // Thread work is written for the verified owner, or before verification
        // provisionally for the selected machine. A composer whose saved work is
        // not usable now stays closed, so skipping it loses nothing.
        let access = self.access();
        let verified = matches!(access, Access::Verified(_));
        let mut conflict = None;
        let mut created = Vec::new();
        for summary in self.host.connection.threads() {
            let Some(composer) = self.workspace.composers.get(&summary.id) else {
                continue;
            };
            if !verified && !next.accepts(summary, access) {
                continue;
            }
            let composer = composer.read(cx);
            let (text, revision, submitted) = composer.saved_draft();
            let title = next
                .threads
                .get(&summary.id)
                .map(|thread| thread.title.clone())
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    self.host
                        .thread
                        .snapshot
                        .as_ref()
                        .filter(|snapshot| snapshot.id == summary.id)
                        .and_then(|snapshot| {
                            snapshot
                                .messages
                                .iter()
                                .find(|message| message.role == MessageRole::User)
                        })
                        .map(|message| session::title(&message.text))
                })
                .unwrap_or_default();
            if let Err(error) = next.update_thread(
                summary.id.clone(),
                Thread {
                    checkout: summary.checkout.clone(),
                    model: summary.model.clone(),
                    title,
                    draft: Draft {
                        images: composer.saved_images().to_vec(),
                        text: text.to_owned(),
                        revision,
                        submitted: submitted.clone(),
                        creation: None,
                    },
                    pending: self.host.submissions.pending(&summary.id).cloned(),
                    pending_submissions: self
                        .host
                        .submissions
                        .all(&summary.id)
                        .iter()
                        .skip(1)
                        .cloned()
                        .collect(),
                    queued_submissions: self.host.submissions.accepted(&summary.id).to_vec(),
                    approval: self.thread_approval(&summary.id),
                    host: None,
                    machine: None,
                },
                access,
            ) {
                conflict = Some(error);
            } else {
                created.push(summary.id.clone());
            }
        }
        // Submission ownership survives parking and a background first-send
        // acknowledgement. Only adopted threads' sends are in memory.
        for (id, thread) in next
            .threads
            .iter_mut()
            .filter(|(id, thread)| self.session.adopted.contains(*id) && thread.usable(access))
        {
            thread.pending = self.host.submissions.pending(id).cloned();
            thread.pending_submissions = self
                .host
                .submissions
                .all(id)
                .iter()
                .skip(1)
                .cloned()
                .collect();
            thread.queued_submissions = self.host.submissions.accepted(id).to_vec();
        }
        if let Some(error) = conflict {
            self.session.note = Some(error);
        }
        self.session.adopted.extend(created);
        if *data == next {
            return;
        }
        let removed_image = data
            .image_ids()
            .any(|id| !next.image_ids().any(|next| next == id));
        if let Some(journal) = &self.session.journal {
            journal.queue(next.clone());
        }
        self.session.data = Some(next);
        self.schedule_session_save(removed_image, cx);
    }

    pub(super) fn clear_new_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(data) = &mut self.session.data {
            if let Some(id) = &self.draft.id {
                data.clear_draft_tab(id);
            }
            if let Some(journal) = &self.session.journal {
                journal.queue(data.clone());
            }
            self.schedule_session_save(true, cx);
        }
    }

    pub(super) fn schedule_session_save(&mut self, immediately: bool, cx: &mut Context<Self>) {
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let timer = cx.background_executor().timer(if immediately {
            Duration::ZERO
        } else {
            Duration::from_millis(250)
        });
        self.session.save = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let result = cx
                .background_executor()
                .spawn(async move { journal.flush() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.session.note = result
                    .err()
                    .map(|error| format!("Workspace not saved: {error}"));
                cx.notify();
            });
        }));
    }

    /// The host's title, else the one saved from its first message.
    fn chosen_title(&self, summary: &ThreadSummary) -> Option<String> {
        summary
            .title
            .as_ref()
            .filter(|title| !title.is_empty())
            .cloned()
            .or_else(|| {
                self.saved_thread(summary)
                    .map(|thread| thread.title.clone())
                    .filter(|title| !title.is_empty())
            })
    }

    pub(super) fn summary_title(&self, summary: &ThreadSummary) -> String {
        self.chosen_title(summary)
            .unwrap_or_else(|| UNTITLED.to_owned())
    }

    pub(super) fn thread_title(&self, id: &str) -> String {
        self.thread_index(id)
            .map(|index| self.summary_title(&self.host.connection.threads()[index]))
            .unwrap_or_else(|| UNTITLED.to_owned())
    }

    /// A thread's name in a control's label. Untitled threads all read "New
    /// thread", so theirs adds the start of the ID to tell them apart.
    pub(super) fn thread_name(&self, id: &str) -> String {
        match self
            .thread_index(id)
            .map(|index| &self.host.connection.threads()[index])
            .and_then(|summary| self.chosen_title(summary))
        {
            Some(title) => title,
            None => format!("{UNTITLED} {}", id.chars().take(8).collect::<String>()),
        }
    }

    /// The name a rename starts from: empty for an untitled thread.
    pub(super) fn rename_text(&self, id: &str) -> String {
        self.host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .and_then(|summary| self.chosen_title(summary))
            .unwrap_or_default()
    }

    pub(super) fn record_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        let id = snapshot.id.clone();
        if !self.session_accepts_thread(&id) {
            return;
        }
        let accepted = self.host.submissions.observe(snapshot);
        let accepted_any = !accepted.is_empty();
        for pending in accepted {
            if let Some(composer) = self.workspace.composers.get(&id) {
                composer.update(cx, |composer, cx| {
                    composer.acknowledge(&pending.text, pending.draft_revision, cx);
                });
            }
        }
        let title_missing = self
            .session
            .data
            .as_ref()
            .and_then(|data| data.threads.get(&id))
            .is_none_or(|thread| thread.title.is_empty());
        let pending_changed = self
            .session
            .data
            .as_ref()
            .and_then(|data| data.threads.get(&id))
            .is_some_and(|thread| {
                thread.pending.as_ref() != self.host.submissions.pending(&id)
                    || thread.pending_submissions.as_slice()
                        != self.host.submissions.all(&id).get(1..).unwrap_or_default()
                    || thread.queued_submissions.as_slice() != self.host.submissions.accepted(&id)
            });
        if title_missing || pending_changed || accepted_any {
            self.capture_session(cx);
        }
    }
}
